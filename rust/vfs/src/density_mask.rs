//! Immutable screen-space demand for density planning. Coarse plan regions
//! may grow or merge; this mask remains an independent proof of occlusion.

#[derive(Clone, Debug, PartialEq)]
pub struct DensityMask {
    view: [f64; 4],
    width: u32,
    height: u32,
    words: usize,
    bits: Vec<u64>,
    // Prefix sum of nonempty 64-pixel words, one row per device row.
    prefix: Vec<u64>,
}

impl DensityMask {
    pub fn new(view: [f64; 4], width: u32, height: u32, mut bits: Vec<u64>) -> Result<Self, String> {
        if width == 0 || height == 0 || !view.iter().all(|v| v.is_finite()) || view[2] <= view[0] || view[3] <= view[1] || !(view[2] - view[0]).is_finite() || !(view[3] - view[1]).is_finite() {
            return Err("invalid density mask view".into());
        }
        let words = (width as usize).div_ceil(64);
        let len = words.checked_mul(height as usize).ok_or("density mask size overflow")?;
        if bits.len() != len { return Err("invalid density mask word count".into()); }
        if width % 64 != 0 {
            let inside = (1u64 << (width % 64)) - 1;
            for row in 0..height as usize { bits[row * words + words - 1] &= inside; }
        }
        let stride = words + 1;
        let mut prefix = vec![0u64; stride.checked_mul(height as usize + 1).ok_or("density mask prefix overflow")?];
        for row in 0..height as usize {
            let mut sum = 0u64;
            for col in 0..words {
                sum += u64::from(bits[row * words + col] != 0);
                prefix[(row + 1) * stride + col + 1] = prefix[row * stride + col + 1].saturating_add(sum);
            }
        }
        Ok(Self { view, width, height, words, bits, prefix })
    }

    pub fn is_empty(&self) -> bool { self.prefix.last().copied().unwrap_or(0) == 0 }

    /// Conservative support query. Invalid/overflowing projections cannot
    /// prove a skip. Boundary expansion protects floating point rounding.
    pub fn world_box_has_open(&self, world: [f64; 4]) -> bool {
        if !world.iter().all(|v| v.is_finite()) { return true; }
        let sx = self.width as f64 / (self.view[2] - self.view[0]);
        let sy = self.height as f64 / (self.view[3] - self.view[1]);
        let x0 = (world[0] - self.view[0]) * sx;
        let x1 = (world[2] - self.view[0]) * sx;
        let y0 = (self.view[3] - world[3]) * sy;
        let y1 = (self.view[3] - world[1]) * sy;
        if ![x0, x1, y0, y1].iter().all(|v| v.is_finite() && v.abs() < (1u64 << 48) as f64) { return true; }
        let c0 = (x0.min(x1) - 1e-7).floor().clamp(0.0, self.width as f64) as usize;
        let c1 = (x0.max(x1) + 1e-7).ceil().clamp(0.0, self.width as f64) as usize;
        let r0 = (y0.min(y1) - 1e-7).floor().clamp(0.0, self.height as f64) as usize;
        let r1 = (y0.max(y1) + 1e-7).ceil().clamp(0.0, self.height as f64) as usize;
        if c0 >= c1 || r0 >= r1 { return false; }
        let (w0, w1) = (c0 / 64, (c1 - 1) / 64 + 1);
        let stride = self.words + 1;
        let count = i128::from(self.prefix[r1 * stride + w1]) + i128::from(self.prefix[r0 * stride + w0])
            - i128::from(self.prefix[r0 * stride + w1]) - i128::from(self.prefix[r1 * stride + w0]);
        if count == 0 { return false; }
        for row in r0..r1 {
            for word in w0..w1 {
                let lo = c0.max(word * 64) - word * 64;
                let hi = c1.min(word * 64 + 64) - word * 64;
                let span = (!0u64 >> (64 - (hi - lo))) << lo;
                if self.bits[row * self.words + word] & span != 0 { return true; }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mask_checks_sparse_words_and_ignores_padding() {
        let mut bits = vec![0; 6];
        bits[3] = 1 << 6;
        bits[5] = !0u64 << 7;
        let mask = DensityMask::new([0., 0., 71., 3.], 71, 3, bits).unwrap();
        assert!(!mask.is_empty());
        assert!(mask.world_box_has_open([70., 1., 71., 2.]));
        assert!(!mask.world_box_has_open([0., 0., 40., 3.]));
        assert!(!mask.world_box_has_open([100., 100., 110., 110.]));
        assert!(mask.world_box_has_open([f64::NAN, 0., 1., 1.]));
        assert!(DensityMask::new([0., 0., 1., 1.], 1, 1, vec![2]).unwrap().is_empty());
    }
}

/// A frame's exact layer unions for child-BVH nodes lacking stored masks.
/// Shared by its layer plans. Failures are cached too: an oversized node
/// must not repeat a bounded metadata scan on every layer. One frame keeps
/// its index mappings alive throughout the memo's lifetime.
#[derive(Debug)]
pub struct DensityLayerMemo {
    nodes: Vec<std::sync::Mutex<std::collections::HashMap<(usize, usize, u32), Option<std::sync::Arc<[u8]>>>>>,
}

impl Default for DensityLayerMemo {
    fn default() -> Self {
        Self { nodes: (0..64).map(|_| std::sync::Mutex::new(std::collections::HashMap::new())).collect() }
    }
}

impl PartialEq for DensityLayerMemo {
    fn eq(&self, other: &Self) -> bool { std::ptr::eq(self, other) }
}

impl DensityLayerMemo {
    pub fn new() -> Self { Self::default() }

    pub(crate) fn has_layer(&self, v: &floe_ovm::Ovm, ni: u32, visible: &[u8]) -> Option<bool> {
        use floe_ovm::{masks_intersect, LMASK_UNKNOWN};
        const MAX_BYTES: usize = 16 << 20;
        const MAX_ENTRIES: usize = 65_536;
        const MAX_READS: usize = 4096;
        let key = (v.data.as_ptr() as usize, v.data.len(), ni);
        let mut nodes = self.nodes[ni as usize % self.nodes.len()].lock().unwrap_or_else(|error| error.into_inner());
        if let Some(known) = nodes.get(&key) {
            return known.as_ref().map(|bits| masks_intersect(bits, visible));
        }
        let capacity = (MAX_BYTES / (v.bs_width.saturating_add(96)).max(1)).min(MAX_ENTRIES) / self.nodes.len();
        if nodes.len() >= capacity { return None; }
        // The bounded metadata scan holds one of 64 shard locks. Different
        // nodes can be built/read in parallel; a node is built just once.
        let mut bits = vec![0u8; v.bs_width];
        let mut pending = vec![ni];
        let mut reads = 0usize;
        let mut complete = true;
        while let Some(at) = pending.pop() {
            reads += 1;
            if reads > MAX_READS || at >= v.n_bvh { complete = false; break; }
            let node = v.bvh(at);
            if node.lmask_rec != LMASK_UNKNOWN {
                for (out, have) in bits.iter_mut().zip(v.bitset(node.lmask_rec)) { *out |= have; }
            } else if node.leaf {
                if reads.saturating_add(node.count as usize) > MAX_READS { complete = false; break; }
                reads += node.count as usize;
                for offset in 0..u64::from(node.count) {
                    let place = u64::from(node.first) + offset;
                    if place >= v.n_places { complete = false; break; }
                    let child = v.place_child(place);
                    if child >= v.n_cells { complete = false; break; }
                    for (out, have) in bits.iter_mut().zip(v.bitset(v.cell_lmask_rec(child))) { *out |= have; }
                }
                if !complete { break; }
            } else {
                if pending.len().saturating_add(node.count as usize) > MAX_READS { complete = false; break; }
                pending.extend((0..u32::from(node.count)).map(|offset| node.first.saturating_add(offset)));
            }
        }
        let made = complete.then(|| std::sync::Arc::<[u8]>::from(bits));
        let answer = made.as_ref().map(|bits| masks_intersect(bits, visible));
        nodes.insert(key, made);
        answer
    }
}
