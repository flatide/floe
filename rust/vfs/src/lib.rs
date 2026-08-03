//! VFS runtime (rust/VFS.md, V2): plan a viewport against design.ovm
//! without touching geometry, keep a working-set session, and build
//! delta OASIS files by splicing page payload bytes verbatim.

pub mod coverage;
pub mod hier;

use floe_ovm::{BBox, Ovm};
use floe_tiler::Xf;

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};

pub struct Vfs {
    pub ovm: Ovm,
    ovp_path: String,
}

#[derive(Clone, Debug)]
pub struct ViewReq {
    /// dbu, closed box
    pub view: BBox,
    /// features (and subtrees) smaller than this are culled
    pub cut_dbu: i64,
    /// visible-layer bitset (ovm layer order, bs_width bytes)
    pub vis: Vec<u8>,
    /// semantic depth limit (u32::MAX = full)
    pub depth: u32,
    /// screen scale (pixels per dbu) for the M7 LOD density rule;
    /// 0.0 disables the rule (probes, kill switch)
    pub px_per_dbu: f64,
}

/// one placement of a page cell in the working-set top. na/nb/va/vb
/// carry a regular array so a cell array (SRAM bitcells, fill) stays
/// ONE placement instead of exploding into a Mat per member; single
impl Vfs {
    pub fn open(dir: &str) -> Result<Vfs, String> {
        let ovm = Ovm::open(&format!("{}/design.ovm", dir))?;
        // viewer-side verification (VFS_HIER.md par.3.6): the marker
        // (design.ovm, structurally validated by from_bytes) commits
        // a specific ovp byte length - a mismatched pair is a broken
        // build or a mixed cache, never something to read through
        let ovp_path = format!("{}/design.ovp", dir);
        let ovp_size = std::fs::metadata(&ovp_path)
            .map_err(|e| format!("{}: {}", ovp_path, e))?
            .len();
        if ovp_size != ovm.ovp_len {
            return Err(format!(
                "corrupt cache; rebuild: design.ovp is {} bytes but \
                 design.ovm committed {}",
                ovp_size, ovm.ovp_len
            ));
        }
        Ok(Vfs { ovm, ovp_path })
    }

    pub fn page_name(&self, pi: u32) -> String {
        let p = self.ovm.page(pi);
        if p.lod == floe_ovm::LOD_EXACT {
            floe_ovm::page_cell_name(p.cell, p.layer_idx, p.seq)
        } else {
            // the variant's payload cell carries the "q" name; WC
            // references and evict lists must match it or klayout
            // materializes an EMPTY exact-named ghost on read
            floe_ovm::lod_cell_name(p.cell, p.layer_idx, p.seq)
        }
    }

    /// visible-layer bitset from "l/d" or name specs (None = all)
    pub fn layer_mask(
        &self,
        specs: Option<&[String]>,
    ) -> Result<Vec<u8>, String> {
        let mut vis = vec![0u8; self.ovm.bs_width];
        match specs {
            None => vis.iter_mut().for_each(|b| *b = 0xff),
            Some(list) => {
                for spec in list {
                    let mut hit = false;
                    for li in 0..self.ovm.n_layers {
                        let lr = self.ovm.layer(li);
                        if lr.name == *spec
                            || format!("{}/{}", lr.layer, lr.dt)
                                == *spec
                        {
                            floe_ovm::bit_set(
                                &mut vis,
                                li as usize,
                            );
                            hit = true;
                        }
                    }
                    if !hit {
                        return Err(format!(
                            "layer {:?} not found",
                            spec
                        ));
                    }
                }
            }
        }
        Ok(vis)
    }

    /// page payload files read in ovp file order (sequential IO)
    pub(crate) fn read_page_payloads(
        &self,
        pages: &[u32],
    ) -> Result<Vec<Vec<u8>>, String> {
        if pages.is_empty() {
            // authored-only incremental delta: no ovp IO at all
            return Ok(Vec::new());
        }
        let mut f = std::fs::File::open(&self.ovp_path)
            .map_err(|e| format!("{}: {}", self.ovp_path, e))?;
        let mut payloads: Vec<Vec<u8>> =
            Vec::with_capacity(pages.len());
        let mut order: Vec<u32> = pages.to_vec();
        order.sort_unstable_by_key(|&pi| {
            self.ovm.page(pi).file_off
        });
        for &pi in &order {
            let p = self.ovm.page(pi);
            let mut buf = vec![0u8; p.csize as usize];
            f.seek(SeekFrom::Start(p.file_off))
                .map_err(|e| e.to_string())?;
            f.read_exact(&mut buf).map_err(|e| e.to_string())?;
            payloads.push(buf);
        }
        Ok(payloads)
    }

}

pub(crate) fn xf_bbox(xf: &Xf, b: &BBox) -> BBox {
    if b.is_empty() {
        return *b;
    }
    let a = xf.apply(b.x0, b.y0);
    let c = xf.apply(b.x1, b.y1);
    BBox {
        x0: a.0.min(c.0),
        y0: a.1.min(c.1),
        x1: a.0.max(c.0),
        y1: a.1.max(c.1),
    }
}

// ------------------------------------------------------- session
/// One in-flight response awaiting the client's ack. Held as a PURE
/// DIFF against `committed` - nothing is applied until the ack
/// arrives, so rollback (stale drop: client never applied the
/// response) is simply discarding this.
#[derive(Debug)]
struct PendingTxn {
    gen: u64,
    /// (page, decoded bytes) - byte sizes ride IN the diff so a
    /// rollback leaves no residue in the committed maps
    new: Vec<(u32, u64)>,
    touch: Vec<u32>,
    evict: Vec<u32>,
    new_bytes: u64,
    evict_bytes: u64,
}

#[derive(Debug, Default)]
pub struct HierUpdate {
    pub new: Vec<u32>,
    pub evict: Vec<u32>,
    pub committed_bytes: u64,
    pub projected_bytes: u64,
    pub pending_new_bytes: u64,
    pub pending_evict_bytes: u64,
    /// stream budget capped this response; the client should ack
    /// and re-request the same view to keep filling
    pub partial: bool,
    /// pages deferred by the cap (telemetry)
    pub deferred: u64,
}

/// Working-set ledger with an ack-gen transaction (VFS_HIER.md
/// par.3.7). The flat Session registers residency at RESPONSE time,
/// which combined with the client's stale-drop leaves permanent
/// silent blanks; here new/evict/LRU-touch stay pending until the
/// next request acks the generation. Request order is serial:
/// resolve_ack (or reset) runs BEFORE apply, so at most one pending
/// exists and planning always sees pure committed state.
pub struct HierSession {
    committed: HashMap<u32, u64>, // page -> last-used gen
    bytes: HashMap<u32, u64>,
    committed_bytes: u64,
    pub budget_bytes: u64,
    pending: Option<PendingTxn>,
    last_gen: u64,
}

impl HierSession {
    pub fn new(budget_bytes: u64) -> HierSession {
        HierSession {
            committed: HashMap::new(),
            bytes: HashMap::new(),
            committed_bytes: 0,
            budget_bytes,
            pending: None,
            last_gen: 0,
        }
    }

    /// full ledger wipe (client rebuilt its mosaic after a partial
    /// apply; reset=1). Generation monotonicity survives - a failed
    /// gen number is never reused.
    pub fn reset(&mut self) {
        self.committed.clear();
        self.bytes.clear();
        self.committed_bytes = 0;
        self.pending = None;
    }

    /// ack semantics (par.3.7): ack == pending.gen -> commit;
    /// ack < pending.gen -> rollback (client dropped the response);
    /// no pending -> idempotent no-op for ack <= last seen gen;
    /// anything from the future is a protocol error.
    pub fn resolve_ack(&mut self, ack: u64) -> Result<(), String> {
        if ack > self.last_gen {
            return Err(format!(
                "ack {} ahead of last gen {}",
                ack, self.last_gen
            ));
        }
        let Some(p) = self.pending.take() else {
            return Ok(()); // dup ack after commit: no-op
        };
        if ack == p.gen {
            for &(pi, b) in &p.new {
                self.committed.insert(pi, p.gen);
                self.bytes.insert(pi, b);
            }
            self.committed_bytes += p.new_bytes;
            for &pi in &p.touch {
                self.committed.insert(pi, p.gen);
            }
            for &pi in &p.evict {
                self.committed.remove(&pi);
                let b = self.bytes.remove(&pi).unwrap_or(0);
                self.committed_bytes -= b;
            }
            Ok(())
        } else {
            // stale drop: the client never applied gen p.gen - its
            // layout still matches `committed`, so drop the diff
            // whole (new re-plans as new, evict stays resident,
            // touches never happened)
            Ok(())
        }
    }

    /// plan bookkeeping: compute new/evict against committed state,
    /// record as pending. `gen` must be strictly monotonic.
    ///
    /// stream_budget (DECODED bytes - page usize_, the parse cost
    /// the client actually pays; 0 = unlimited) caps how much NEW
    /// payload one response carries - the progressive first paint:
    /// the client parses ~budget bytes, acks, re-requests the same
    /// view, and `plan - committed` naturally shrinks until the
    /// response stops saying partial. `prio` is the planner's
    /// per-page priority aligned with `plan_pages` (local-frame
    /// center distance, HierPlan::page_prio); ties break on page
    /// index - deterministic.
    pub fn apply(
        &mut self,
        ovm: &Ovm,
        plan_pages: &[u32],
        prio: &[u64],
        gen: u64,
        stream_budget: u64,
    ) -> Result<HierUpdate, String> {
        if gen <= self.last_gen {
            return Err(format!(
                "gen {} not monotonic (last {})",
                gen, self.last_gen
            ));
        }
        if self.pending.is_some() {
            return Err("pending txn unresolved (no ack)".into());
        }
        self.last_gen = gen;
        let mut cand: Vec<(u64, u32, u64)> = Vec::new(); // prio,pi,usize
        let mut touch = Vec::new();
        for (k, &pi) in plan_pages.iter().enumerate() {
            if self.committed.contains_key(&pi) {
                touch.push(pi);
            } else {
                let p = ovm.page(pi);
                let pr = prio.get(k).copied().unwrap_or(u64::MAX);
                cand.push((pr, pi, p.usize_ as u64));
            }
        }
        let mut partial = false;
        let mut deferred = 0u64;
        if stream_budget > 0 {
            let total: u64 = cand.iter().map(|c| c.2).sum();
            if total > stream_budget {
                cand.sort_unstable();
                // strict prefix: priority order is also load order;
                // the first page always ships (progress guarantee)
                let mut cut = cand.len();
                let mut usum = 0u64;
                for (i, c) in cand.iter().enumerate() {
                    if i > 0 && usum + c.2 > stream_budget {
                        cut = i;
                        break;
                    }
                    usum += c.2;
                }
                deferred = (cand.len() - cut) as u64;
                partial = deferred > 0;
                cand.truncate(cut);
            }
        }
        let mut new: Vec<(u32, u64)> = Vec::new();
        let mut new_bytes = 0u64;
        for (_, pi, ub) in cand {
            new.push((pi, ub));
            new_bytes += ub;
        }
        // eviction against the PROJECTED size (committed + this
        // response's new), never evicting this plan's pages - the
        // v1 "insert then evict overflow" behavior, two-phased
        let mut evict = Vec::new();
        let mut evict_bytes = 0u64;
        let mut projected = self.committed_bytes + new_bytes;
        if projected > self.budget_bytes {
            let current: HashSet<u32> =
                plan_pages.iter().copied().collect();
            let mut cand: Vec<(u64, u32)> = self
                .committed
                .iter()
                .filter(|(pi, _)| !current.contains(pi))
                .map(|(&pi, &g)| (g, pi))
                .collect();
            cand.sort_unstable();
            for (_, pi) in cand {
                if projected <= self.budget_bytes {
                    break;
                }
                let b = *self.bytes.get(&pi).unwrap_or(&0);
                projected -= b;
                evict_bytes += b;
                evict.push(pi);
            }
        }
        let upd = HierUpdate {
            new: new.iter().map(|&(pi, _)| pi).collect(),
            evict: evict.clone(),
            committed_bytes: self.committed_bytes,
            projected_bytes: projected,
            pending_new_bytes: new_bytes,
            pending_evict_bytes: evict_bytes,
            partial,
            deferred,
        };
        self.pending = Some(PendingTxn {
            gen,
            new,
            touch,
            evict,
            new_bytes,
            evict_bytes,
        });
        Ok(upd)
    }

    /// test hook: ledger metadata entries must track committed pages
    /// exactly (a rollback leaves no residue)
    #[cfg(test)]
    fn bytes_entries(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_committed(&self, pi: u32) -> bool {
        self.committed.contains_key(&pi)
    }
    pub fn committed_bytes(&self) -> u64 {
        self.committed_bytes
    }
    pub fn committed_pages(&self) -> usize {
        self.committed.len()
    }
    pub fn last_gen(&self) -> u64 {
        self.last_gen
    }
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_oasis::write::{write_tree, WCell};

    /// one cell, n pages of the given decoded sizes - just enough
    /// metadata for the session ledger
    fn sess_ovm(usizes: &[u64]) -> Ovm {
        let mut b = floe_ovm::Builder::new(1000.0, 0, 0, 1);
        b.top = 0;
        b.layer(1, 0, "L", 0, 0);
        let m = b.bitset(&[1]);
        let bbx = BBox { x0: 0, y0: 0, x1: 10, y1: 10 };
        for (k, &u) in usizes.iter().enumerate() {
            // pages sit at x = k*1000 so streaming priority is
            // testable; csize == usize for budget math
            let pb = BBox {
                x0: k as i64 * 1000,
                y0: 0,
                x1: k as i64 * 1000 + 10,
                y1: 10,
            };
            b.page(0, 0, k as u32, &pb, 0, u, u, 1, 1, 10, 10, floe_ovm::LOD_EXACT, floe_ovm::LOD_PAGE_NONE);
        }
        let pr = b.prange(
            0,
            0,
            usizes.len() as u32,
            floe_ovm::PBVH_NONE,
        );
        b.cell(
            "C",
            0,
            0,
            &bbx,
            &bbx,
            0,
            0,
            0,
            usizes.len() as u32,
            0,
            0,
            pr,
            1,
            m,
            m,
            1,
        );
        Ovm::from_bytes(b.finish(1 << 20)).unwrap()
    }

    #[test]
    fn hier_session_commit_and_stale_drop() {
        let ovm = sess_ovm(&[10, 10, 10]);
        let mut s = HierSession::new(1 << 30);
        // gen1 pending, then commit via ack
        let u = s.apply(&ovm, &[0, 1], &[0; 2], 1, 0).unwrap();
        assert_eq!(u.new, vec![0, 1]);
        assert_eq!(u.committed_bytes, 0);
        assert_eq!(u.pending_new_bytes, 20);
        assert!(s.has_pending());
        s.resolve_ack(1).unwrap();
        assert_eq!(s.committed_bytes(), 20);
        // stale drop: gen2 response never applied (ack stays 1) -
        // the ledger must forget it and RESEND the pages (the flat
        // path's permanent-blank bug, par.3.7 regression gate)
        let u2 = s.apply(&ovm, &[0, 1, 2], &[0; 3], 2, 0).unwrap();
        assert_eq!(u2.new, vec![2]);
        s.resolve_ack(1).unwrap(); // rollback gen2
        assert_eq!(s.committed_bytes(), 20);
        // rollback leaves NO metadata residue (pending is a pure
        // diff - review finding: bytes entries once accumulated)
        assert_eq!(s.bytes_entries(), 2);
        let u3 = s.apply(&ovm, &[0, 1, 2], &[0; 3], 3, 0).unwrap();
        assert_eq!(u3.new, vec![2], "dropped page must re-send");
        s.resolve_ack(3).unwrap();
        assert_eq!(s.committed_bytes(), 30);
        // dup ack after commit: idempotent no-op
        s.resolve_ack(3).unwrap();
        s.resolve_ack(1).unwrap();
        assert_eq!(s.committed_bytes(), 30);
    }

    #[test]
    fn hier_session_protocol_errors() {
        let ovm = sess_ovm(&[10, 10]);
        let mut s = HierSession::new(1 << 30);
        s.apply(&ovm, &[0], &[0; 1], 1, 0).unwrap();
        // ack from the future: error, pending survives
        assert!(s.resolve_ack(2).is_err());
        assert!(s.has_pending());
        s.resolve_ack(1).unwrap();
        // plan without resolving is impossible in the serve order,
        // but the guard must hold
        s.apply(&ovm, &[1], &[0; 1], 2, 0).unwrap();
        assert!(s.apply(&ovm, &[1], &[0; 1], 3, 0).is_err());
        s.resolve_ack(2).unwrap();
        // non-monotonic gen
        assert!(s.apply(&ovm, &[1], &[0; 1], 2, 0).is_err());
        // reset wipes the ledger but keeps gen monotonicity
        s.reset();
        assert_eq!(s.committed_bytes(), 0);
        assert!(s.apply(&ovm, &[0], &[0; 1], 2, 0).is_err(), "gen reuse");
        let u = s.apply(&ovm, &[0, 1], &[0; 2], 9, 0).unwrap();
        assert_eq!(u.new, vec![0, 1], "reset resends all");
    }

    #[test]
    fn hier_session_projected_budget_and_touch_rollback() {
        // budget 100: committed 90 + new 30 must evict BEFORE the
        // layout ever holds 120 (projected rule, par.3.7)
        let ovm = sess_ovm(&[90, 30, 10, 10]);
        let mut s = HierSession::new(100);
        s.apply(&ovm, &[0], &[0; 1], 1, 0).unwrap();
        s.resolve_ack(1).unwrap();
        let u = s.apply(&ovm, &[1], &[0; 1], 2, 0).unwrap();
        assert_eq!(u.new, vec![1]);
        assert_eq!(u.evict, vec![0]);
        assert_eq!(u.pending_evict_bytes, 90);
        assert_eq!(u.projected_bytes, 30);
        assert_eq!(u.committed_bytes, 90, "not yet committed");
        s.resolve_ack(2).unwrap();
        assert_eq!(s.committed_bytes(), 30);
        // LRU touch is pending too: a rolled-back touch must not
        // save a page from eviction
        let ovm = sess_ovm(&[10, 10, 10]);
        let mut s = HierSession::new(25);
        s.apply(&ovm, &[0, 1], &[0; 2], 1, 0).unwrap();
        s.resolve_ack(1).unwrap();
        // gen2 touches p0 (plan [0]) but the client drops it
        s.apply(&ovm, &[0], &[0; 1], 2, 0).unwrap();
        s.resolve_ack(1).unwrap(); // rollback: touch undone
        // gen3 brings p2: 20 + 10 > 25 -> evict ONE, LRU order;
        // with the touch rolled back p0 and p1 tie on gen1 and the
        // deterministic (gen, page) order evicts p0
        let u = s.apply(&ovm, &[2], &[0; 1], 3, 0).unwrap();
        assert_eq!(u.evict, vec![0]);
        s.resolve_ack(3).unwrap();
        // same sequence but gen2 COMMITTED: p0 was touched (gen2),
        // so p1 is the LRU victim
        let ovm2 = sess_ovm(&[10, 10, 10]);
        let mut s2 = HierSession::new(25);
        s2.apply(&ovm2, &[0, 1], &[0; 2], 1, 0).unwrap();
        s2.resolve_ack(1).unwrap();
        s2.apply(&ovm2, &[0], &[0; 1], 2, 0).unwrap();
        s2.resolve_ack(2).unwrap(); // commit: touch applied
        let u2 = s2.apply(&ovm2, &[2], &[0; 1], 3, 0).unwrap();
        assert_eq!(u2.evict, vec![1]);
    }

    #[test]
    fn hier_session_stream_budget() {
        // planner priorities: page2 is the screen center (prio 0),
        // then page1, then page0 -> strict prefix per round,
        // natural convergence. Budget is DECODED bytes (usize_).
        let ovm = sess_ovm(&[10, 10, 10]);
        let prio = &[9u64, 4, 0];
        let mut s = HierSession::new(1 << 30);
        let u = s.apply(&ovm, &[0, 1, 2], prio, 1, 10).unwrap();
        assert_eq!(u.new, vec![2], "smallest prio first");
        assert!(u.partial);
        assert_eq!(u.deferred, 2);
        s.resolve_ack(1).unwrap();
        let u = s.apply(&ovm, &[0, 1, 2], prio, 2, 10).unwrap();
        assert_eq!(u.new, vec![1]);
        assert!(u.partial);
        s.resolve_ack(2).unwrap();
        let u = s.apply(&ovm, &[0, 1, 2], prio, 3, 10).unwrap();
        assert_eq!(u.new, vec![0]);
        assert!(!u.partial);
        assert_eq!(u.deferred, 0);
        s.resolve_ack(3).unwrap();
        assert_eq!(s.committed_bytes(), 30);
        assert!(s.is_committed(2) && s.is_committed(0));
        // a dropped partial round re-sends the SAME chunk (rollback
        // + deterministic priority)
        let ovm = sess_ovm(&[10, 10, 10]);
        let mut s = HierSession::new(1 << 30);
        let u = s.apply(&ovm, &[0, 1, 2], prio, 1, 10).unwrap();
        assert_eq!(u.new, vec![2]);
        s.resolve_ack(0).unwrap(); // stale drop
        let u = s.apply(&ovm, &[0, 1, 2], prio, 2, 10).unwrap();
        assert_eq!(u.new, vec![2], "rollback then same chunk");
        // oversized single page still ships (progress guarantee)
        let ovm = sess_ovm(&[100, 10]);
        let mut s = HierSession::new(1 << 30);
        let u = s.apply(&ovm, &[0, 1], &[0, 5], 1, 10).unwrap();
        assert_eq!(u.new, vec![0], "first page always ships");
        assert!(u.partial);
        // budget 0 = unlimited (probe / default-off)
        let ovm = sess_ovm(&[10, 10, 10]);
        let mut s = HierSession::new(1 << 30);
        let u = s.apply(&ovm, &[0, 1, 2], prio, 1, 0).unwrap();
        assert_eq!(u.new.len(), 3);
        assert!(!u.partial);
    }

}
