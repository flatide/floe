use super::*;
use crate::drc::analysis::scope::Runs;
use crate::drc::changes::{stamp_at, STAMP};
use crate::drc::desktop::{Left, Review, STATUS_NONE, STATUS_WAIVED};
use crate::drc::team::flags::FlagStore;
use crate::drc::team::stats::{collect_from, collect_with};
use crate::drc::Pack;
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;

static SERIAL: AtomicU64 = AtomicU64::new(0);

/// The errors of the fixture's one rule that has any - rule 1; rules 0 and
/// 2 are empty: three whole chunks and a short one.
const N: u64 = 3 * CHUNK + 100;
const RULE: usize = 1;

struct Folder {
    dir: PathBuf,
    pack: PathBuf,
}

impl Folder {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "floe-team-live-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let pack = dir.join(".results.db.tray");
        fs::write(&pack, crate::drc::tests::bytes(N)).unwrap();
        Self { dir, pack }
    }
    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
    fn cache(&self) -> PathBuf {
        self.file(".results.db.team")
    }
    fn log(&self, who: &str) -> PathBuf {
        self.file(&format!(".results.db.changes.{who}"))
    }
    fn id(&self) -> Identity {
        let stop = AtomicUsize::new(0);
        Identity::of_pack(&Pack::open(&self.pack, &stop).unwrap(), "statistics").unwrap()
    }
    /// The kept counts after a merge at `now` with the grace `grace`.
    fn merged(&self, now: u64, grace: u64) -> Kept {
        let (stop, id) = (AtomicUsize::new(0), self.id());
        let found = discover(&self.pack).unwrap();
        merge_at(&self.pack, &id, &found, now, grace, true, &stop).unwrap()
    }
    /// `merged` that is to keep nothing (FLOE_TEAM_KEEP=off).
    fn counted(&self, now: u64) -> Kept {
        let (stop, id) = (AtomicUsize::new(0), self.id());
        let found = discover(&self.pack).unwrap();
        merge_at(&self.pack, &id, &found, now, 10, false, &stop).unwrap()
    }
    /// The exact report's team: a row a rule, and its reviewers.
    fn exact(&self) -> (Vec<[u64; 8]>, usize) {
        let stop = AtomicUsize::new(0);
        let pack = Pack::open(&self.pack, &stop).unwrap();
        let id = Identity::of_pack(&pack, "statistics").unwrap();
        let report = collect_from(&pack, &id, discover(&self.pack).unwrap(), None, &stop).unwrap();
        let team = &report["team"];
        let rows = team["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                ["visited", "waived", "not_required", "conflicts", "pending", "confirmed", "unconfirmed", "rejected"]
                    .map(|key| r[key].as_u64().unwrap())
            })
            .collect();
        (rows, team["reviewers"].as_u64().unwrap() as usize)
    }
    /// The exact report, whole.
    fn report(&self) -> serde_json::Value {
        let stop = AtomicUsize::new(0);
        let pack = Pack::open(&self.pack, &stop).unwrap();
        let id = Identity::of_pack(&pack, "statistics").unwrap();
        collect_from(&pack, &id, discover(&self.pack).unwrap(), None, &stop).unwrap()
    }
    /// The exact report's team counts a rule, as it counted them.
    fn exact_teams(&self) -> Vec<Team> {
        let stop = AtomicUsize::new(0);
        let pack = Pack::open(&self.pack, &stop).unwrap();
        let id = Identity::of_pack(&pack, "statistics").unwrap();
        collect_with(&pack, &id, discover(&self.pack).unwrap(), None, &stop).unwrap().1
    }
    /// The kept counts are the exact report's - its report's numbers, and
    /// every count a rule as it was counted (the ones only the brief report
    /// says among them) - and cover everything.
    fn agree(&self, kept: &Kept, what: &str) {
        assert_eq!((rows(kept, &self.id()), kept.reviewers), self.exact(), "{what}");
        assert_eq!(kept.rules, self.exact_teams(), "{what}: every count");
        assert!(kept.current && kept.waiting.is_empty(), "{what}: {:?}", kept.waiting);
    }
    fn records(&self, who: &str) -> Vec<changes::Record> {
        let identity: [u8; 28] = self.id().waive_header[12..40].try_into().unwrap();
        changes::read(&self.log(who), &identity, None).unwrap().records
    }
    /// Say `records` (kind, rule, a, b) in `who`'s log as their session would.
    fn say(&self, who: &str, records: &[(u32, u32, u64, u64)]) {
        let mut bytes = Vec::new();
        for &(kind, rule, a, b) in records {
            bytes.extend(kind.to_le_bytes());
            bytes.extend(rule.to_le_bytes());
            bytes.extend(a.to_le_bytes());
            bytes.extend(b.to_le_bytes());
        }
        File::options().append(true).open(self.log(who)).unwrap().write_all(&bytes).unwrap();
    }
    /// A waive sidecar of this result with nothing waived, made by hand.
    fn sidecar(&self, who: &str) -> PathBuf {
        let id = self.id();
        let path = self.file(&format!(".results.db.waive.{who}"));
        let mut bytes = id.waive_header.to_vec();
        bytes.resize(40 + id.total() as usize + 4 * id.sizes.len(), 0);
        fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// The fixture's rule with errors, as `rows` says it.
fn rows_of(kept: &Kept, id: &Identity) -> [u64; 8] {
    rows(kept, id)[RULE]
}

fn rows(kept: &Kept, id: &Identity) -> Vec<[u64; 8]> {
    kept.rules
        .iter()
        .zip(&id.sizes)
        .map(|(t, &size)| {
            [
                t.visited,
                t.waived,
                t.not_required,
                t.conflicts,
                size - t.done,
                t.confirmed,
                t.waived_any - t.confirmed,
                t.rejected,
            ]
        })
        .collect()
}

/// A file as a merge that wrote nothing leaves it: the same file, time, size.
fn state(path: &Path) -> Option<(u64, u64, u64)> {
    fs::metadata(path).ok().map(|m| (m.ino(), stamp_of(&m), m.len()))
}

/// See that `path`'s time is past `before`, whatever the clock's grain is:
/// what a writer that changed the file a moment later leaves.
fn moved(path: &Path, before: u64) {
    while stamp_at(path) <= before {
        std::thread::sleep(Duration::from_millis(2));
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now())
            .unwrap();
    }
}

/// Set rule 1's error `error` to `value` in the waive sidecar `path` as a
/// writer that says nothing would - the status byte and the rule's counter -
/// and leave the file's time at `stamp` (None: the time of the write).
fn poke(path: &Path, id: &Identity, error: u64, value: u8, stamp: Option<u64>) {
    let before = stamp_at(path);
    let file = File::options().read(true).write(true).open(path).unwrap();
    let at = 40 + id.starts[RULE] + error;
    let mut old = [0u8];
    file.read_exact_at(&mut old, at).unwrap();
    file.write_all_at(&[value], at).unwrap();
    let counter = 40 + id.total() + 4 * RULE as u64;
    let mut raw = [0u8; 4];
    file.read_exact_at(&mut raw, counter).unwrap();
    let count = i64::from(u32::from_le_bytes(raw)) + i64::from(value == STATUS_WAIVED) - i64::from(old[0] == STATUS_WAIVED);
    file.write_all_at(&(count as u32).to_le_bytes(), counter).unwrap();
    drop(file);
    match stamp {
        Some(ns) => File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_nanos(ns))
            .unwrap(),
        None => moved(path, before),
    }
}

/// A reviewer's session as the service opens it: the review, and its flags
/// saying their changes in the review's log.
struct Session {
    review: Review,
    flags: FlagStore,
}

impl Session {
    fn open(f: &Folder, who: &str) -> Self {
        let stop = AtomicUsize::new(0);
        let mut review = Review::open(&f.pack, None, Some(who), &stop).unwrap();
        assert!(!review.read_only(), "{:?}", review.notices);
        // (which waives are locked is looked for at each ask here)
        review.set_lock_refresh(std::time::Duration::ZERO);
        let id = Identity::of(&review).unwrap();
        let flags = FlagStore::new(&f.pack, id.sizes.clone(), id.fingerprint.clone(), who).unwrap();
        flags.attach_changes(Arc::clone(&review.changes));
        Self { review, flags }
    }
    fn waive(&mut self, errors: &[u64], on: bool) {
        let value = if on { STATUS_WAIVED } else { STATUS_NONE };
        self.review.set_status(RULE, errors, value).unwrap();
    }
    fn waive_run(&mut self, a: u64, b: u64, on: bool) {
        let (stop, value) = (AtomicUsize::new(0), if on { STATUS_WAIVED } else { STATUS_NONE });
        self.review
            .set_status_members(RULE, &Runs::new(vec![a], vec![b]), value, &stop, &mut |_, _| {}, &mut Left::default())
            .unwrap();
    }
    fn visit(&self, error: u64) {
        self.flags.mark_visited(RULE, error).unwrap();
    }
    fn exclude(&self, a: u64, b: u64, on: bool) {
        let stop = AtomicUsize::new(0);
        self.flags
            .set_members(RULE, Some(&Runs::new(vec![a], vec![b])), on, &stop, &mut |_, _, _| {})
            .unwrap();
    }
    /// Confirm errors `a..b` (whoever has them waived, or nobody), or take
    /// the confirmation back.
    fn confirm(&mut self, a: u64, b: u64, on: bool) {
        let errors: Vec<u64> = (a..b).collect();
        self.review.confirm_set(RULE, &errors, on).unwrap();
    }
}

#[test]
fn what_a_log_and_its_files_say_against_what_is_kept() {
    let tail = |log_id: u64, records: &[(u32, u64, u64)]| changes::Tail {
        log_id,
        len: changes::HEADER + changes::RECORD * records.len() as u64,
        records: records
            .iter()
            .map(|&(kind, a, b)| changes::Record { kind, rule: 1, a, b })
            .collect(),
    };
    let kept = Seen {
        log_id: 7,
        cursor: 48,
        log_stamp: [100, 200, 0, 0],
        stamps: [100, 200, 0, 0],
        suspect: 0,
        known: 3,
        active: BTreeSet::new(),
    };
    let nothing = tail(7, &[]);
    let word = tail(7, &[(TOUCH, 0, 0), (STAMP, 150, 200)]);
    let check = |kept: Option<&Seen>, tail: Option<&changes::Tail>, known: u8, stamps: Stamps, want: Verdict, what: &str| {
        assert_eq!(judge(kept, tail, known, stamps), want, "{what}");
    };
    use Verdict::{Again, Ahead, Said, Unsaid};
    check(None, None, 1, [5, 0, 0, 0], Again, "new to the counts");
    check(None, Some(&word), 3, [150, 200, 0, 0], Again, "new to the counts, with a log");
    check(Some(&kept), Some(&nothing), 3, [100, 200, 0, 0], Said, "nothing changed");
    check(Some(&kept), Some(&nothing), 1, [100, 0, 0, 0], Again, "their flags went");
    check(Some(&kept), Some(&word), 3, [150, 200, 0, 0], Said, "as the last word left them");
    check(Some(&kept), Some(&word), 3, [160, 200, 0, 0], Ahead, "the statuses newer than the last word");
    check(Some(&kept), Some(&word), 3, [150, 210, 0, 0], Ahead, "the flags newer than the last word");
    check(Some(&kept), Some(&word), 3, [100, 200, 0, 0], Unsaid, "the log says more than the files show");
    check(Some(&kept), Some(&word), 3, [160, 190, 0, 0], Unsaid, "one file newer, one not there yet");
    check(Some(&kept), Some(&nothing), 3, [110, 200, 0, 0], Unsaid, "changed, and nothing said");
    check(Some(&kept), Some(&nothing), 3, [90, 200, 0, 0], Unsaid, "changed backwards, and nothing said");
    // a settle half seen: what was touched without its stamp yet is judged
    // against the last stamp known
    let half = tail(7, &[(TOUCH, 0, 0)]);
    check(Some(&kept), Some(&half), 3, [100, 200, 0, 0], Said, "touched, the stamp not seen yet, the files as before");
    check(Some(&kept), Some(&half), 3, [110, 200, 0, 0], Ahead, "touched, the stamp not seen yet");
    check(Some(&kept), Some(&tail(7, &[(RESET, 0, 0), (STAMP, 150, 200)])), 3, [150, 200, 0, 0], Again, "a RESET");
    check(Some(&kept), Some(&tail(8, &[(STAMP, 100, 200)])), 3, [100, 200, 0, 0], Again, "another log");
    check(Some(&kept), None, 3, [100, 200, 0, 0], Again, "their log is gone");
    // a reviewer without a log has nothing to wait for
    let silent = Seen { log_id: 0, cursor: 0, log_stamp: [0; 4], ..kept.clone() };
    check(Some(&silent), None, 3, [100, 200, 0, 0], Said, "no log, nothing changed");
    check(Some(&silent), None, 3, [100, 201, 0, 0], Again, "no log, changed");
    check(Some(&silent), Some(&tail(7, &[(STAMP, 100, 200)])), 3, [100, 200, 0, 0], Again, "their first log");
    // counted of files older than the log had said: any change counts again
    let behind = Seen { stamps: [90, 200, 0, 0], ..kept.clone() };
    check(Some(&behind), Some(&nothing), 3, [90, 200, 0, 0], Said, "behind, nothing changed");
    check(Some(&behind), Some(&nothing), 3, [100, 200, 0, 0], Again, "behind, the files came to what was said");
    check(Some(&behind), Some(&word), 3, [150, 200, 0, 0], Again, "behind, more said");
    check(Some(&behind), Some(&word), 3, [90, 200, 0, 0], Again, "behind, more said and the files as they were");
    // the confirm file is a file of theirs like the other two (0.12.466)
    let confirming = Seen { log_stamp: [100, 200, 50, 0], stamps: [100, 200, 50, 0], known: 7, ..kept.clone() };
    let cword = tail(7, &[(TOUCH, 0, 0), (STAMP, 100, 200), (changes::CSTAMP, 60, 0)]);
    check(Some(&confirming), Some(&nothing), 7, [100, 200, 50, 0], Said, "confirming, nothing changed");
    check(Some(&confirming), Some(&nothing), 7, [100, 200, 60, 0], Unsaid, "confirmed, and nothing said");
    check(Some(&confirming), Some(&cword), 7, [100, 200, 60, 0], Said, "confirmed, as the last word left it");
    check(Some(&confirming), Some(&cword), 7, [100, 200, 50, 0], Unsaid, "the log says a confirmation the file does not show");
    check(Some(&confirming), Some(&cword), 7, [100, 200, 70, 0], Ahead, "the confirm file newer than the last word");
    check(Some(&confirming), Some(&word), 7, [150, 200, 50, 0], Said, "a waive said, the confirm file as it was");
    check(Some(&kept), Some(&cword), 7, [100, 200, 60, 0], Again, "their first confirm file");
    check(Some(&confirming), Some(&nothing), 3, [100, 200, 0, 0], Again, "their confirm file went");
    // the rejects log is the fourth (0.12.468): its time is the second
    // number of the confirm file's record
    let rejecting = Seen { log_stamp: [100, 200, 50, 30], stamps: [100, 200, 50, 30], known: 15, ..kept.clone() };
    let rword = tail(7, &[(TOUCH, 0, 0), (STAMP, 100, 200), (changes::CSTAMP, 50, 40)]);
    check(Some(&rejecting), Some(&nothing), 15, [100, 200, 50, 30], Said, "rejecting, nothing changed");
    check(Some(&rejecting), Some(&nothing), 15, [100, 200, 50, 40], Unsaid, "rejected, and nothing said");
    check(Some(&rejecting), Some(&rword), 15, [100, 200, 50, 40], Said, "rejected, as the last word left it");
    check(Some(&rejecting), Some(&rword), 15, [100, 200, 50, 30], Unsaid, "the log says a rejection the file does not show");
    check(Some(&rejecting), Some(&rword), 15, [100, 200, 50, 45], Ahead, "the rejects log newer than the last word");
    check(Some(&confirming), Some(&rword), 15, [100, 200, 50, 40], Again, "their first rejects log");
    check(Some(&rejecting), Some(&nothing), 7, [100, 200, 50, 0], Again, "their rejects log went");
}

#[test]
fn the_cache_file_is_whole_and_this_results_or_no_cache() {
    let mut cache = Cache::empty(3);
    cache.set(1, 0, [1, 2, 3, 0, 4, 2, 1, 0, 3]);
    cache.set(1, 7, [4096, 0, 0, 0, 4096, 0, 0, 0, 0]);
    cache.set(2, 1, [0, 5, 0, 0, 0, 5, 5, 0, 0]);
    assert_eq!(cache.rules, [[0; 9], [4097, 2, 3, 0, 4100, 2, 1, 0, 3], [0, 5, 0, 0, 0, 5, 5, 0, 0]]);
    // a chunk's counts replaced, and one that counts nothing any more
    cache.set(1, 0, [2, 2, 0, 1, 2, 3, 0, 0, 1]);
    cache.set(2, 1, [0; 9]);
    assert_eq!(cache.rules, [[0; 9], [4098, 2, 0, 1, 4098, 3, 0, 0, 1], [0; 9]]);
    assert_eq!((cache.kept(1), cache.kept(2), cache.chunks.len()), (vec![0, 7], vec![], 2));
    assert_eq!(
        cache.teams()[1],
        Team { visited: 4098, waived: 2, not_required: 0, conflicts: 1, done: 4098, waived_any: 3, confirmed: 0, rejected: 0, not_required_any: 1 }
    );
    cache.reviewers.insert(
        "kim".into(),
        Seen {
            log_id: 9,
            cursor: 72,
            log_stamp: [5, 6, 10, 0],
            stamps: [7, 8, 11, 0],
            suspect: 3,
            known: 3,
            active: [1u32, 2].into(),
        },
    );
    cache.reviewers.insert("김".into(), Seen::default());
    let bytes = cache.bytes("abc");
    assert_eq!(Cache::of(&bytes, "abc", 3), Some(cache.clone()));
    assert_eq!(Cache::of(&Cache::empty(0).bytes(""), "", 0), Some(Cache::empty(0)));
    // another result's, another number of rules
    assert!(Cache::of(&bytes, "abd", 3).is_none() && Cache::of(&bytes, "abc", 4).is_none());
    // any byte changed, the file cut short, or grown
    for at in 0..bytes.len() {
        let mut changed = bytes.clone();
        changed[at] ^= 0x40;
        assert!(Cache::of(&changed, "abc", 3).is_none(), "byte {at}");
    }
    for len in [0, 7, bytes.len() - 1] {
        assert!(Cache::of(&bytes[..len], "abc", 3).is_none(), "{len} bytes");
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(Cache::of(&longer, "abc", 3).is_none());
    // sums that are right over counts that cannot be
    let mut wrong = cache.clone();
    wrong.rules[1][0] += 1;
    assert!(Cache::of(&wrong.bytes("abc"), "abc", 3).is_none(), "a rule's total is not its chunks'");
    let mut wrong = cache.clone();
    wrong.chunks.insert((1, 3), [0; 9]);
    assert!(Cache::of(&wrong.bytes("abc"), "abc", 3).is_none(), "a chunk that counts nothing");
    let mut wrong = cache.clone();
    wrong.chunks.insert((1, 3), [4097, 0, 0, 0, 0, 0, 0, 0, 0]);
    wrong.rules[1][0] += 4097;
    assert!(Cache::of(&wrong.bytes("abc"), "abc", 3).is_none(), "a count past a chunk");
    let mut wrong = cache.clone();
    wrong.chunks.insert((3, 0), [1, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert!(Cache::of(&wrong.bytes("abc"), "abc", 3).is_none(), "a rule the result has not");
    // the names
    assert_eq!(path_of(Path::new("/r/.x.db.tray")).unwrap(), Path::new("/r/.x.db.team"));
    let key = lock_key(Path::new("/r/.x.db.team"));
    assert_eq!(
        (key.dir.as_path(), key.name.as_str(), key.subject.as_str()),
        (Path::new("/r/.floe-lock"), "x.db.team", "the team's counts of x.db")
    );
    assert_eq!(changes::CHUNK, CHUNK, "the change log's chunk is the flags'");
}

/// The kept counts are what the exact report counts from every status byte
/// and bit plane, after each thing a reviewer's session does - and a poll
/// with nothing changed writes nothing and takes no lock.
#[test]
fn the_kept_counts_are_the_exact_ones_after_every_change() {
    let f = Folder::new();
    let id = f.id();
    // nobody has reviewed: nothing to keep, nothing made
    let kept = f.merged(1000, 10);
    f.agree(&kept, "nobody");
    assert!(kept.stored && kept.reviewers == 0 && kept.skipped.is_empty());
    assert!(!f.cache().exists() && !f.file(".floe-lock").exists());
    // a session that has done nothing is a reviewer with nothing
    let mut kim = Session::open(&f, "kim");
    let kept = f.merged(1000, 10);
    f.agree(&kept, "a session");
    assert_eq!(kept.reviewers, 1);
    kim.waive(&[0, 1, 2, CHUNK + 5, N - 1], true);
    let kept = f.merged(1000, 10);
    f.agree(&kept, "clicks");
    assert_eq!(rows(&kept, &id)[RULE][..5], [0, 5, 0, 0, N]);
    // nothing changed since: the file is not written again, no lock taken
    let lock = f.file(".floe-lock/results.db.team.build");
    let before = (state(&f.cache()), state(&lock));
    assert!(before.0.is_some() && before.1.is_some());
    let kept = f.merged(1001, 10);
    f.agree(&kept, "unchanged");
    assert_eq!((state(&f.cache()), state(&lock), kept.counted), (before.0, before.1, 0));
    // (the kept file is made with the pack's mode, whatever the umask)
    assert_eq!(
        fs::metadata(f.cache()).unwrap().permissions().mode() & 0o777,
        fs::metadata(&f.pack).unwrap().permissions().mode() & 0o666 | 0o600
    );
    // asked to keep nothing (FLOE_TEAM_KEEP=off): what changed is counted
    // for the answer, each time, and neither the file nor the lock touched
    kim.waive(&[3, 2 * CHUNK + 7], true);
    for now in [1001, 1002] {
        let kept = f.counted(now);
        assert_eq!((rows(&kept, &id), kept.reviewers), f.exact(), "nothing kept, at {now}");
        assert!(kept.current && !kept.stored && kept.counted == 2, "{} counted", kept.counted);
        assert_eq!((state(&f.cache()), state(&lock)), before);
    }
    let kept = f.merged(1002, 10);
    f.agree(&kept, "kept again");
    assert!(kept.stored && kept.counted == 2);
    kim.waive(&[3, 2 * CHUNK + 7], false);
    f.agree(&f.merged(1002, 10), "as before");
    // two who visited the same error and judged it differently: a conflict,
    // and no waive of the team's; an error only one of them visited is none
    let lee = Session::open(&f, "lee");
    kim.visit(1);
    lee.visit(1);
    lee.visit(CHUNK + 5);
    let kept = f.merged(1002, 10);
    f.agree(&kept, "a conflict");
    assert_eq!(rows(&kept, &id)[RULE][..5], [2, 4, 0, 1, N - 2]);
    assert_eq!(kept.reviewers, 2);
    // an exclusion across chunks, a selection waived by a job, parts of
    // both taken back
    lee.exclude(CHUNK - 10, 2 * CHUNK + 10, true);
    f.agree(&f.merged(1003, 10), "an exclusion");
    kim.waive_run(100, 3 * CHUNK + 50, true);
    f.agree(&f.merged(1004, 10), "a job's waive");
    kim.waive_run(CHUNK, 2 * CHUNK, false);
    lee.exclude(CHUNK, CHUNK + 1, false);
    kim.waive(&[0], false);
    f.agree(&f.merged(1005, 10), "taken back");
    // a saved waive file loaded over the review: everything is counted again
    let saved = f.file("saved.waive");
    kim.review.waive_export(&saved).unwrap();
    kim.waive_run(0, N, false);
    let kept = f.merged(1006, 10);
    f.agree(&kept, "all taken back");
    assert_eq!(rows(&kept, &id)[RULE][1], 0);
    let said = f.records("kim").len();
    kim.review.waive_import(&saved).unwrap();
    kim.review.sync_waives().unwrap();
    assert_eq!(f.records("kim")[said].kind, RESET);
    let kept = f.merged(1007, 10);
    f.agree(&kept, "a saved file loaded");
    assert!(rows(&kept, &id)[RULE][1] > CHUNK);
    // whatever three reviewers do, in whatever order - park a confirmer,
    // who confirms what is waived and what is not and takes it back, and
    // rejects and takes that back (0.12.468); a reviewer's session looks
    // now and then, and waives the rejected waives it finds again, or
    // leaves them rejected (0.12.469)
    let mut park = Session::open(&f, "park");
    park.review.set_confirmer(true);
    let mut people = [kim, lee, park];
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    let (mut conflicts, mut excluded, mut counted) = (0, 0, 0);
    // (a reviewer's first flags, first confirmation and first word in a
    // change log make a file, and a file that comes has its reviewer's
    // rules counted whole, once)
    let (mut confirmed, mut rejected, mut returned) = (0, 0, 0);
    let files = |person: usize| {
        let name = ["kim", "lee", "park"][person];
        [
            f.file(&format!(".results.db.review.{name}.sqlite3")).exists(),
            f.file(&format!(".results.db.confirm.{name}")).exists(),
            f.file(&format!(".results.db.rejects.{name}")).exists(),
            f.log(name).exists(),
        ]
    };
    for step in 0..300 {
        let person = next(3) as usize;
        let who = &mut people[person];
        // (half of the time near a chunk's start, where the others are too)
        let a = if next(2) == 0 { next(4) * CHUNK + next(24) } else { next(N) };
        let most = if next(4) == 0 { 6000 } else { 40 };
        let b = (a + 1 + next(most)).min(N);
        // (park confirms or rejects a time in three; anyone looks for
        // rejections of their waives a time in ten - and answers them, or
        // not)
        let action = if person == 2 && next(3) == 0 {
            8 + next(5)
        } else if next(10) == 0 {
            13
        } else {
            next(8)
        };
        let had = files(person);
        match action {
            0 => who.waive(&[a], true),
            1 => who.waive(&[a], false),
            2 => who.waive_run(a, b, true),
            3 => who.waive_run(a, b, false),
            4 | 5 => who.visit(a),
            6 => who.exclude(a, b, true),
            7 => who.exclude(a, b, false),
            8 => who.confirm(a, a + 1, true),
            9 => who.confirm(a, b.min(a + 200), true),
            10 => who.confirm(a, b.min(a + 200), false),
            11 => who.review.reject_set(RULE, &[(a, b.min(a + 200))], "no").unwrap(),
            12 => drop(who.review.reject_drop(RULE, &[(a, b.min(a + 200))]).unwrap()),
            _ => {
                let looked = who.review.look().unwrap();
                for back in &looked.rejected {
                    if next(3) > 0 {
                        returned += who.review.resubmit(RULE, &back.runs).unwrap();
                    }
                }
            }
        }
        let kept = f.merged(2000 + step, 10);
        f.agree(&kept, &format!("step {step}"));
        // what is counted again is what changed: the chunk of a click, the
        // chunks of a run - not the rule
        let b = if action >= 9 { b.min(a + 200) } else { b };
        let most = match action {
            _ if files(person) != had => N.div_ceil(CHUNK),
            0 | 1 | 4 | 5 | 8 => 1,
            // (the waives waived again lie wherever a rejection does)
            13 => N.div_ceil(CHUNK),
            _ => (b - 1) / CHUNK - a / CHUNK + 1,
        };
        assert!(
            kept.counted <= most,
            "step {step}: {} chunks counted, {most} changed (reviewer {person}, action {action}, errors {a}..{b})",
            kept.counted
        );
        counted += kept.counted;
        confirmed = confirmed.max(kept.rules[RULE].confirmed);
        rejected = rejected.max(kept.rules[RULE].rejected);
        conflicts = conflicts.max(kept.rules[RULE].conflicts);
        excluded = excluded.max(kept.rules[RULE].not_required);
    }
    assert!(conflicts > 0 && excluded > 0 && confirmed > 0, "{conflicts} conflicts, {excluded} set aside, {confirmed} confirmed");
    assert!(rejected > 0 && returned > 0, "{rejected} rejected, {returned} waives waived again");
    assert!(counted >= 150, "{counted} chunks counted in 300 steps");
    assert_eq!(f.merged(2300, 10).reviewers, 3);
    // a session that ends and one that starts say nothing more: the log's
    // last stamp is how the files stand
    let names = ["kim", "lee", "park"];
    let said: Vec<usize> = names.iter().map(|who| f.records(who).len()).collect();
    drop(people);
    for (who, &said) in names.iter().zip(&said) {
        drop(Session::open(&f, who));
        assert_eq!(f.records(who).len(), said, "{who}");
    }
    let before = state(&f.cache());
    f.agree(&f.merged(3000, 10), "sessions closed and opened");
    assert_eq!(state(&f.cache()), before);
}

/// A confirmer reads the team's waives - who waived each error and why -
/// and confirms them in a file of their own; the lists, the exact report
/// and the kept counts say the same.
#[test]
fn the_teams_waives_are_listed_and_confirmed_by_the_confirmer() {
    use crate::drc::reasons::{runs_of, Kind as Reason};
    use crate::drc::team::waives::{confirm_chunk, Counts as Waives, Kind, TeamWaives};
    let f = Folder::new();
    let id = f.id();
    let stop = AtomicUsize::new(0);
    let view = || TeamWaives::open(&f.pack, &f.id()).unwrap();
    let counts = |waived, confirmed| Waives { waived, confirmed, unconfirmed: waived - confirmed, rejected: 0 };
    // nobody has waived anything: nothing to confirm
    let mut v = view();
    assert_eq!(v.counts(None, &stop).unwrap(), counts(0, 0));
    assert_eq!(v.page(RULE, Kind::Waived, 0, 10, &stop).unwrap(), (0, vec![]));
    assert!(v.reviewers().is_empty() && v.confirmers().is_empty());
    let mut kim = Session::open(&f, "kim");
    let mut lee = Session::open(&f, "lee");
    let mut hong = Session::open(&f, "hong");
    kim.waive(&[0, 1, 2, CHUNK + 5], true);
    kim.review.reason_set(Reason::Waive, RULE, &runs_of(&[1, 2]), "dummy fill").unwrap();
    lee.waive(&[2, 3], true);
    lee.review.reason_set(Reason::Waive, RULE, &runs_of(&[2]), "seal ring").unwrap();
    // the team's waives: an error two reviewers waived is one
    let mut v = view();
    assert_eq!(v.reviewers(), ["hong", "kim", "lee"]);
    assert!(v.confirmers().is_empty() && v.skipped.is_empty());
    assert_eq!(v.counts(Some(RULE), &stop).unwrap(), counts(5, 0));
    assert_eq!(v.counts(None, &stop).unwrap(), counts(5, 0));
    assert_eq!(v.counts(Some(0), &stop).unwrap(), counts(0, 0));
    assert!(v.counts(Some(9), &stop).is_err());
    let all = vec![0, 1, 2, 3, CHUNK + 5];
    assert_eq!(v.page(RULE, Kind::Unconfirmed, 0, 10, &stop).unwrap(), (5, all.clone()));
    assert_eq!(v.page(RULE, Kind::Waived, 1, 2, &stop).unwrap(), (5, vec![1, 2]));
    assert_eq!(v.page(RULE, Kind::Waived, 4, 9, &stop).unwrap(), (5, vec![CHUNK + 5]));
    assert_eq!(v.page(RULE, Kind::Waived, 9, 9, &stop).unwrap(), (5, vec![]));
    assert_eq!(v.page(RULE, Kind::Confirmed, 0, 10, &stop).unwrap(), (0, vec![]));
    // who waived, and why as each of them said
    let rows = v.rows(RULE, &[0, 2, 3, 7]).unwrap();
    let who = |i: usize| rows[i].waived_by.iter().map(String::as_str).collect::<Vec<_>>();
    let why = |i: usize| rows[i].reasons.iter().map(|w| (w.reviewer.as_str(), w.text.as_str())).collect::<Vec<_>>();
    assert_eq!((who(0), why(0)), (vec!["kim"], vec![]));
    assert_eq!((who(1), why(1)), (vec!["kim", "lee"], vec![("kim", "dummy fill"), ("lee", "seal ring")]));
    assert_eq!((who(2), why(2)), (vec!["lee"], vec![]));
    assert!(who(3).is_empty() && rows.iter().all(|r| r.confirmed_by.is_empty()));
    assert!(rows[1].reasons[0].time > 1_600_000_000 && v.rows(RULE, &[N]).is_err());
    // only a session started as a confirmer confirms
    assert_eq!(confirm_chunk(&mut hong.review, &mut v, RULE, 0, None, true).unwrap_err().kind, ErrorKind::Unsupported);
    assert_eq!(hong.review.confirm_set(RULE, &[0], true).unwrap_err().kind, ErrorKind::Unsupported);
    let file = f.file(".results.db.confirm.hong");
    assert!(!file.exists() && !hong.review.confirmer());
    hong.review.set_confirmer(true);
    // every waive of a chunk: the four somebody waived, no other error
    assert_eq!(confirm_chunk(&mut hong.review, &mut v, RULE, 0, None, true).unwrap(), (4, 0));
    hong.review.sync_waives().unwrap();
    assert!(file.exists() && hong.review.confirms.counts()[RULE] == 4);
    // errors named one by one: the waived one is confirmed, the other said
    let named = Runs::new(vec![CHUNK + 5], vec![CHUNK + 7]);
    assert_eq!(confirm_chunk(&mut hong.review, &mut v, RULE, 1, Some(&named), true).unwrap(), (1, 1));
    assert_eq!(confirm_chunk(&mut hong.review, &mut v, RULE, 1, Some(&named), true).unwrap(), (0, 1));
    assert_eq!(confirm_chunk(&mut hong.review, &mut v, RULE, 9, None, true).unwrap(), (0, 0));
    hong.review.sync_waives().unwrap();
    let mut v = view();
    assert_eq!(v.confirmers(), ["hong"]);
    assert_eq!(v.counts(Some(RULE), &stop).unwrap(), counts(5, 5));
    assert_eq!(v.page(RULE, Kind::Confirmed, 0, 10, &stop).unwrap(), (5, all.clone()));
    assert_eq!(v.page(RULE, Kind::Unconfirmed, 0, 10, &stop).unwrap(), (0, vec![]));
    assert_eq!(v.rows(RULE, &[2]).unwrap()[0].confirmed_by, ["hong"]);
    let kept = f.merged(1000, 10);
    f.agree(&kept, "confirmed");
    assert_eq!(rows_of(&kept, &id)[5..7], [5, 0]);
    // a confirmed waive is locked: its reviewer's session does not turn it
    // back (0.12.467), and nothing is counted anew
    assert_eq!(kim.review.set_status(RULE, &[0, 3], STATUS_NONE).unwrap().locked, [0], "3 is lee's waive, not kim's");
    assert_eq!(view().counts(Some(RULE), &stop).unwrap(), counts(5, 5));
    assert_eq!(f.merged(1001, 10).counted, 0);
    // turned back all the same - by a session with the role, or a build
    // before the lock - it takes its confirmation out of the count; the
    // confirmer's file still has it, and waived again, by anybody, it
    // stands again: locked at once
    kim.review.set_confirmer(true);
    assert!(kim.review.set_status(RULE, &[0], STATUS_NONE).unwrap().is_empty());
    kim.review.set_confirmer(false);
    assert_eq!(view().counts(Some(RULE), &stop).unwrap(), counts(4, 4));
    let kept = f.merged(1001, 10);
    f.agree(&kept, "a confirmed waive turned back");
    assert_eq!((rows_of(&kept, &id)[5..7].to_vec(), hong.review.confirms.counts()[RULE]), (vec![4, 0], 5));
    lee.waive(&[0], true);
    assert_eq!(view().counts(Some(RULE), &stop).unwrap(), counts(5, 5));
    f.agree(&f.merged(1002, 10), "waived again");
    assert_eq!(lee.review.set_status(RULE, &[0], STATUS_NONE).unwrap().locked, [0]);
    // the confirmer takes one back; a second confirmer's same error is the
    // same error
    assert_eq!(hong.review.confirm_set(RULE, &[1, 7], false).unwrap(), 1);
    let mut v = view();
    assert_eq!(v.counts(Some(RULE), &stop).unwrap(), counts(5, 4));
    assert_eq!(v.page(RULE, Kind::Unconfirmed, 0, 10, &stop).unwrap(), (1, vec![1]));
    let mut yoon = Session::open(&f, "yoon");
    yoon.review.set_confirmer(true);
    assert_eq!(yoon.review.confirm_set(RULE, &[1, 2, 9], true).unwrap(), 3);
    let mut v = view();
    assert_eq!(v.confirmers(), ["hong", "yoon"]);
    assert_eq!(v.counts(Some(RULE), &stop).unwrap(), counts(5, 5));
    let rows = v.rows(RULE, &[1, 2, 9]).unwrap();
    assert_eq!(rows[0].confirmed_by, ["yoon"]);
    assert_eq!(rows[1].confirmed_by, ["hong", "yoon"]);
    assert!(rows[2].waived_by.is_empty() && rows[2].confirmed_by == ["yoon"]);
    let kept = f.merged(1003, 10);
    f.agree(&kept, "two confirmers");
    assert_eq!(rows_of(&kept, &id)[5..7], [5, 0]);
    // what each reviewer's row of the exact report says: the errors they
    // confirmed, and of their waives the ones somebody confirmed
    let report = f.report();
    let person = |name: &str| {
        report["reviewers"].as_array().unwrap().iter().find(|r| r["id"] == name).unwrap().clone()
    };
    assert_eq!((person("hong")["confirmed"].as_u64(), person("yoon")["confirmed"].as_u64()), (Some(4), Some(3)));
    assert!(person("kim").get("confirmed").is_none() && person("hong")["confirm_file"] == file.to_str().unwrap());
    assert_eq!(
        (person("kim")["waived_confirmed"].as_u64(), person("lee")["waived_confirmed"].as_u64(), person("hong")["waived_confirmed"].as_u64()),
        (Some(3), Some(3), Some(0))
    );
    assert_eq!((report["team"]["confirmed"].as_u64(), report["team"]["unconfirmed"].as_u64()), (Some(5), Some(0)));
    // a chunk's confirmations taken back whole
    assert_eq!(confirm_chunk(&mut hong.review, &mut v, RULE, 0, None, false).unwrap(), (3, 0));
    hong.review.sync_waives().unwrap();
    let kept = f.merged(1004, 10);
    f.agree(&kept, "taken back");
    assert_eq!(rows_of(&kept, &id)[5..7], [3, 2]);
    // a confirm file of another result is nobody's confirmation of this one
    drop((hong, yoon));
    let mut bytes = fs::read(&file).unwrap();
    bytes[20] ^= 1;
    fs::write(&file, bytes).unwrap();
    let mut v = view();
    assert_eq!((v.confirmers(), v.skipped.len()), (vec!["yoon"], 1));
    assert_eq!(v.counts(Some(RULE), &stop).unwrap(), counts(5, 2));
    let kept = f.merged(1005, 10);
    f.agree(&kept, "a confirm file of another result");
    assert!(kept.skipped.iter().any(|(who, path, _)| who == "hong" && *path == file));
}

/// A rejection is of each reviewer's waive until they waive the error again
/// (0.12.469): the lists, the exact report and the kept counts class an
/// error by its waives that are not rejected, say whose waive is rejected,
/// by whom and why, and what a reviewer's own log keeps of it.
#[test]
fn a_rejected_waive_is_listed_until_its_reviewer_waives_it_again() {
    use crate::drc::reasons::{runs_of, Kind as Reason};
    use crate::drc::team::waives::{confirm_chunk, reject_runs, Counts as Waives, Kind, TeamWaives};
    let f = Folder::new();
    let id = f.id();
    let stop = AtomicUsize::new(0);
    let view = || TeamWaives::open(&f.pack, &f.id()).unwrap();
    let counts = |waived, confirmed, rejected| Waives { waived, confirmed, unconfirmed: waived - confirmed, rejected };
    let mut kim = Session::open(&f, "kim");
    let mut lee = Session::open(&f, "lee");
    let mut hong = Session::open(&f, "hong");
    hong.review.set_confirmer(true);
    kim.waive(&[0, 1, 2], true);
    kim.review.reason_set(Reason::Waive, RULE, &runs_of(&[0, 1, 2]), "dummy fill").unwrap();
    lee.waive(&[2, 3], true);
    lee.review.reason_set(Reason::Waive, RULE, &runs_of(&[2]), "seal ring").unwrap();
    // a rejection is of the errors somebody has waived: the other is said
    let (runs, skipped) = reject_runs(&hong.review, &mut view(), RULE, &[1, 2, 9], true).unwrap();
    assert_eq!((runs.clone(), skipped), (vec![(1, 3)], 1));
    hong.review.reject_set(RULE, &runs, "not a dummy").unwrap();
    // an error whose waives are all rejected is waived by nobody, and
    // listed as that - with whose waive it is, who rejected it and why
    let mut v = view();
    assert_eq!(v.counts(Some(RULE), &stop).unwrap(), counts(2, 0, 2));
    assert_eq!(v.page(RULE, Kind::Waived, 0, 10, &stop).unwrap(), (2, vec![0, 3]));
    assert_eq!(v.page(RULE, Kind::Unconfirmed, 0, 10, &stop).unwrap(), (2, vec![0, 3]));
    assert_eq!(v.page(RULE, Kind::Rejected, 0, 10, &stop).unwrap(), (2, vec![1, 2]));
    assert_eq!(v.rules(&stop).unwrap(), [(RULE, counts(2, 0, 2))]);
    let rows = v.rows(RULE, &[1, 2, 0]).unwrap();
    fn names(list: &[String]) -> Vec<&str> {
        list.iter().map(String::as_str).collect()
    }
    fn by(list: &[crate::drc::team::waives::Why]) -> Vec<(&str, &str)> {
        list.iter().map(|w| (w.reviewer.as_str(), w.text.as_str())).collect()
    }
    assert_eq!((names(&rows[0].waived_by), names(&rows[0].rejected_of), by(&rows[0].rejected_by)), (vec!["kim"], vec!["kim"], vec![("hong", "not a dummy")]));
    assert_eq!((names(&rows[1].waived_by), names(&rows[1].rejected_of), by(&rows[1].rejected_by)), (vec!["kim", "lee"], vec!["kim", "lee"], vec![("hong", "not a dummy")]));
    assert_eq!(by(&rows[1].reasons), [("kim", "dummy fill"), ("lee", "seal ring")], "a rejected waive has its reason");
    assert!(rows[2].rejected_of.is_empty() && rows[2].rejected_by.is_empty() && rows.iter().all(|r| r.history.is_empty()));
    let kept = f.merged(1000, 10);
    f.agree(&kept, "rejected");
    assert_eq!((kept.rules[RULE].waived, kept.rules[RULE].rejected), (2, 2));
    // each reviewer's row of the exact report: of their waives, the rejected
    let report = f.report();
    let person = |report: &serde_json::Value, name: &str| {
        report["reviewers"].as_array().unwrap().iter().find(|r| r["id"] == name).unwrap().clone()
    };
    assert_eq!((person(&report, "kim")["waived"].as_u64(), person(&report, "kim")["waived_rejected"].as_u64()), (Some(3), Some(2)));
    assert_eq!((person(&report, "lee")["waived_rejected"].as_u64(), person(&report, "hong")["rejected"].as_u64()), (Some(1), Some(2)));
    assert_eq!(report["team"]["rejected"].as_u64(), Some(2));
    // a rejected waive is not confirmed: the others of the chunk are
    assert_eq!(confirm_chunk(&mut hong.review, &mut v, RULE, 0, None, true).unwrap(), (2, 0));
    hong.review.sync_waives().unwrap();
    assert_eq!(view().counts(Some(RULE), &stop).unwrap(), counts(2, 2, 2));
    // one of the two reviewers waives the error again: it is waived - by
    // them - and waits for the confirmer; the other's waive is rejected
    // still. What was is kept, and listed
    assert_eq!(kim.review.look().unwrap().rejected[0].runs, [(1, 3)]);
    assert_eq!(kim.review.resubmit(RULE, &[(2, 3)]).unwrap(), 1);
    kim.review.reason_set(Reason::Waive, RULE, &[(2, 3)], "dummy fill, rule note 4").unwrap();
    let mut v = view();
    assert_eq!(v.counts(Some(RULE), &stop).unwrap(), counts(3, 2, 1));
    assert_eq!(v.page(RULE, Kind::Unconfirmed, 0, 10, &stop).unwrap(), (1, vec![2]));
    assert_eq!(v.page(RULE, Kind::Rejected, 0, 10, &stop).unwrap(), (1, vec![1]));
    let row = v.rows(RULE, &[2]).unwrap().remove(0);
    assert_eq!((names(&row.waived_by), names(&row.rejected_of), by(&row.rejected_by)), (vec!["kim", "lee"], vec!["lee"], vec![("hong", "not a dummy")]));
    assert_eq!(by(&row.reasons), [("kim", "dummy fill, rule note 4"), ("lee", "seal ring")]);
    let past: Vec<(&str, &str, &str, &str)> = row
        .history
        .iter()
        .map(|p| (p.reviewer.as_str(), p.event.what, p.event.by.as_str(), p.event.text.as_str()))
        .collect();
    assert_eq!(past, [("kim", "waived", "", "dummy fill"), ("kim", "rejected", "hong", "not a dummy")]);
    assert!(serde_json::to_value(&row).unwrap()["history"][1]["by"] == "hong");
    let kept = f.merged(1001, 10);
    f.agree(&kept, "waived again");
    assert_eq!((kept.rules[RULE].waived, kept.rules[RULE].rejected), (3, 1));
    let report = f.report();
    assert_eq!((person(&report, "kim")["waived_rejected"].as_u64(), person(&report, "lee")["waived_rejected"].as_u64()), (Some(1), Some(1)));
    // rejected again: of the waive that is none - a new rejection, for
    // every reviewer who has the error waived to answer; the error whose
    // waives are all rejected already is not rejected again
    let (runs, skipped) = reject_runs(&hong.review, &mut v, RULE, &[1, 2], true).unwrap();
    assert_eq!((runs.clone(), skipped), (vec![(2, 3)], 1));
    hong.review.reject_set(RULE, &runs, "see me").unwrap();
    let mut v = view();
    assert_eq!(v.counts(Some(RULE), &stop).unwrap(), counts(2, 2, 2));
    let row = v.rows(RULE, &[2]).unwrap().remove(0);
    assert_eq!((names(&row.rejected_of), by(&row.rejected_by)), (vec!["kim", "lee"], vec![("hong", "see me")]));
    assert_eq!(row.history.len(), 2);
    f.agree(&f.merged(1002, 10), "rejected again");
    // taken back: the errors this confirmer's log holds - the waives are
    // as they were, at once, and what was answered stays kept
    let (runs, skipped) = reject_runs(&hong.review, &mut v, RULE, &[0, 1, 2], false).unwrap();
    assert_eq!((runs.clone(), skipped), (vec![(1, 3)], 0));
    assert!(hong.review.reject_drop(RULE, &runs).unwrap());
    let mut v = view();
    assert_eq!(v.counts(Some(RULE), &stop).unwrap(), counts(4, 2, 0));
    assert_eq!(v.page(RULE, Kind::Rejected, 0, 10, &stop).unwrap(), (0, vec![]));
    let row = v.rows(RULE, &[2]).unwrap().remove(0);
    assert!(row.rejected_of.is_empty() && row.rejected_by.is_empty() && row.history.len() == 2);
    let kept = f.merged(1003, 10);
    f.agree(&kept, "taken back");
    assert_eq!((kept.rules[RULE].waived, kept.rules[RULE].rejected), (4, 0));
    assert_eq!(rows_of(&kept, &id)[5..8], [2, 2, 0]);
}

/// The log makes the answer cheap; the files' times are what makes it right:
/// a change no record says - a build before the log, a run that ended
/// between its bits and its word, a record that comes before its bits - is
/// counted, at once where there is nothing to wait for and after the grace
/// where there is.
#[test]
fn a_change_nobody_said_is_found_by_the_files_times() {
    let f = Folder::new();
    let id = f.id();
    // a reviewer of a build before the change log - files, no log: counted
    // again at once, each time they change
    let old = f.sidecar("old");
    poke(&old, &id, 7, STATUS_WAIVED, None);
    poke(&old, &id, 2 * CHUNK, STATUS_WAIVED, None);
    f.agree(&f.merged(1000, 10), "no log");
    poke(&old, &id, 8, STATUS_WAIVED, None);
    let kept = f.merged(1001, 10);
    f.agree(&kept, "no log, changed");
    assert_eq!(rows(&kept, &id)[RULE][1], 3);
    // ... their flags too (a store that says nothing, as that build's)
    let flags = FlagStore::new(&f.pack, id.sizes.clone(), id.fingerprint.clone(), "old").unwrap();
    flags.mark_visited(RULE, 7).unwrap();
    let old_flags = flags.path();
    let kept = f.merged(1002, 10);
    f.agree(&kept, "no log, flags came");
    assert_eq!(rows(&kept, &id)[RULE][0], 1);
    let before = stamp_at(&old_flags);
    flags.mark_visited(RULE, 3 * CHUNK + 1).unwrap();
    moved(&old_flags, before);
    let kept = f.merged(1003, 10);
    f.agree(&kept, "no log, flags changed");
    assert_eq!(rows(&kept, &id)[RULE][0], 2);
    flags.close();
    drop(flags);
    // a reviewer with a log, changed by a writer that says nothing: the
    // record is waited for a grace long - the counts as they were, and said
    // not to be current - then their rules are counted again
    let mut kim = Session::open(&f, "kim");
    kim.waive(&[10, 11], true);
    kim.visit(10);
    let kept = f.merged(1010, 10);
    f.agree(&kept, "a session");
    let was = rows(&kept, &id);
    drop(kim);
    let waive = f.file(".results.db.waive.kim");
    poke(&waive, &id, 12, STATUS_WAIVED, None);
    let kept = f.merged(1020, 10);
    assert!(!kept.current && kept.waiting.len() == 1 && kept.waiting[0].starts_with("kim's review files"), "{:?}", kept.waiting);
    assert!(rows(&kept, &id) == was && was != f.exact().0);
    // (still waiting: nothing is written for it, and no lock taken)
    let before = state(&f.cache());
    let kept = f.merged(1025, 10);
    assert!(!kept.current && rows(&kept, &id) == was);
    assert_eq!(state(&f.cache()), before);
    f.agree(&f.merged(1030, 10), "the grace ended");
    // no grace asked (FLOE_TEAM_GRACE_S=0): at once
    poke(&waive, &id, 13, STATUS_WAIVED, None);
    f.agree(&f.merged(1040, 0), "no grace");
    // the reviewer's next session finds its files changed without a word
    // and says so before anything else: at once, whatever the grace
    poke(&waive, &id, 14, STATUS_WAIVED, None);
    let said = f.records("kim").len();
    let mut kim = Session::open(&f, "kim");
    let now = f.records("kim");
    assert_eq!(now.len(), said + 2);
    assert_eq!((now[said].kind, now[said + 1].kind), (RESET, STAMP));
    f.agree(&f.merged(1050, 10), "a session's RESET");
    // a record lost - the run ended between its bits and its word
    kim.waive(&[20], true);
    let len = fs::metadata(f.log("kim")).unwrap().len();
    File::options()
        .write(true)
        .open(f.log("kim"))
        .unwrap()
        .set_len(len - 2 * changes::RECORD)
        .unwrap();
    assert!(!f.merged(1060, 10).current);
    assert!(!f.merged(1069, 10).current);
    f.agree(&f.merged(1070, 10), "a lost record");
    // a job still going: what its log said so far is counted, the rest
    // waits for its record
    let all = Runs::new(vec![0], vec![N]);
    kim.review.set_status_range(RULE, &all, STATUS_WAIVED, 0, CHUNK, &mut Left::default()).unwrap();
    kim.review.sync_waives().unwrap();
    let said = stamp_at(&waive);
    kim.review.set_status_range(RULE, &all, STATUS_WAIVED, CHUNK, CHUNK, &mut Left::default()).unwrap();
    moved(&waive, said);
    let kept = f.merged(1080, 10);
    assert!(!kept.current && kept.waiting[0].starts_with("kim is changing their review"), "{:?}", kept.waiting);
    let so_far = rows(&kept, &id)[RULE][1];
    assert!(so_far >= CHUNK && so_far < 2 * CHUNK, "{so_far}");
    kim.review.sync_waives().unwrap();
    let kept = f.merged(1081, 10);
    f.agree(&kept, "the job's end");
    assert!(rows(&kept, &id)[RULE][1] >= 2 * CHUNK);
    drop(kim);
    f.agree(&f.merged(1090, 10), "the session closed");
    // the log says more than the files show yet - a record read before its
    // bits can be, as across hosts: nothing of it is taken until the files
    // are as it says
    let kim_flags = stamp_at(&f.file(".results.db.review.kim.sqlite3"));
    assert!(kim_flags > 1);
    let ahead = (stamp_at(&waive) / 1_000_000_000 + 5) * 1_000_000_000;
    let before = rows(&f.merged(1090, 10), &id);
    f.say("kim", &[(TOUCH, RULE as u32, 3, 3), (STAMP, 0, ahead, kim_flags)]);
    let kept = f.merged(1100, 10);
    assert!(!kept.current && rows(&kept, &id) == before);
    poke(&waive, &id, 3 * CHUNK + 50, STATUS_WAIVED, Some(ahead));
    let kept = f.merged(1101, 10);
    f.agree(&kept, "the bits came");
    assert_eq!(rows(&kept, &id)[RULE][1], before[RULE][1] + 1);
    // ... and where they never come, the files are counted as they are after
    // the grace, and again whenever they change
    let further = ahead + 5_000_000_000;
    f.say("kim", &[(TOUCH, RULE as u32, 0, 0), (STAMP, 0, further, kim_flags)]);
    assert!(!f.merged(1110, 10).current);
    f.agree(&f.merged(1120, 10), "the grace of a record without its bits");
    let before = state(&f.cache());
    f.agree(&f.merged(1121, 10), "counted of files older than was said, unchanged");
    assert_eq!(state(&f.cache()), before);
    poke(&waive, &id, 3 * CHUNK + 51, STATUS_WAIVED, Some(further));
    f.agree(&f.merged(1122, 10), "counted of files older than was said, changed");
    // a reviewer's files removed: their bits go with them
    let with = rows(&f.merged(1122, 10), &id);
    fs::remove_file(&old).unwrap();
    fs::remove_file(&old_flags).unwrap();
    let kept = f.merged(1130, 10);
    f.agree(&kept, "a reviewer gone");
    assert!(rows(&kept, &id) != with && kept.reviewers == 1);
    // the cache damaged, or no cache at all: made again
    let mut bytes = fs::read(f.cache()).unwrap();
    let at = bytes.len() - 5;
    bytes[at] ^= 1;
    fs::write(f.cache(), &bytes).unwrap();
    f.agree(&f.merged(1140, 10), "a damaged cache");
    fs::write(f.cache(), b"FLOETEAM and nothing of it").unwrap();
    f.agree(&f.merged(1141, 10), "no cache");
    assert!(Cache::of(&fs::read(f.cache()).unwrap(), &id.fingerprint, id.sizes.len()).is_some());
    // a folder that takes no file: counted for the answer, kept nowhere
    fs::remove_file(f.cache()).unwrap();
    fs::set_permissions(&f.dir, fs::Permissions::from_mode(0o555)).unwrap();
    let refused = File::create(f.file("probe")).is_err();
    let kept = f.merged(1150, 10);
    fs::set_permissions(&f.dir, fs::Permissions::from_mode(0o755)).unwrap();
    f.agree(&kept, "a folder that cannot be written");
    if refused {
        assert!(!kept.stored && !f.cache().exists());
    }
}

/// One merge at a time: a second one that finds the first still at work
/// when its wait ends answers with what is kept, and says so.
#[test]
fn a_merge_that_finds_another_at_work_answers_with_what_is_kept() {
    if lock::disabled() {
        return;
    }
    let f = Folder::new();
    let id = f.id();
    let old = f.sidecar("old");
    poke(&old, &id, 7, STATUS_WAIVED, None);
    let kept = f.merged(1000, 10);
    f.agree(&kept, "kept");
    let was = rows(&kept, &id);
    poke(&old, &id, 8, STATUS_WAIVED, None);
    let held = lock::writer(&lock_key(&f.cache()), Mode::Additive, "another merge").unwrap();
    let started = Instant::now();
    let kept = f.merged(1001, 10);
    let waited = started.elapsed();
    assert!(waited >= WAIT && waited < WAIT * 4, "{waited:?}");
    assert!(
        !kept.current
            && kept.waiting.len() == 1
            && kept.waiting[0].starts_with("the team's counts are being brought up to date by "),
        "{:?}",
        kept.waiting
    );
    assert!(rows(&kept, &id) == was && kept.counted == 0 && was != f.exact().0);
    drop(held);
    let kept = f.merged(1002, 10);
    f.agree(&kept, "the other merge ended");
    assert_eq!(rows(&kept, &id)[RULE][1], 2);
}

/// Files that are not this result's, or do not read, are left out of the
/// counts as the exact report leaves them out, said, and not tried at the
/// cost of a count each time.
#[test]
fn files_that_do_not_read_are_left_out_and_said() {
    let f = Folder::new();
    let id = f.id();
    let bad = f.sidecar("bad");
    poke(&bad, &id, 1, STATUS_WAIVED, None);
    let flags = f.file(".results.db.review.bad.sqlite3");
    fs::write(&flags, b"not a database at all").unwrap();
    let kept = f.merged(1000, 10);
    f.agree(&kept, "flags that do not read");
    assert_eq!(rows(&kept, &id)[RULE][..5], [0, 1, 0, 0, N]);
    assert_eq!(kept.skipped.len(), 1, "{:?}", kept.skipped);
    assert_eq!((kept.skipped[0].0.as_str(), &kept.skipped[0].1), ("bad", &flags));
    let before = state(&f.cache());
    let kept = f.merged(1001, 10);
    f.agree(&kept, "flags that do not read, again");
    assert_eq!((kept.skipped.len(), state(&f.cache())), (1, before));
    // a sidecar of another result is nobody's review of this one
    let other = f.sidecar("other");
    let mut bytes = fs::read(&other).unwrap();
    bytes[20] ^= 1;
    fs::write(&other, bytes).unwrap();
    let kept = f.merged(1002, 10);
    f.agree(&kept, "another result's sidecar");
    assert_eq!((kept.reviewers, kept.skipped.len(), state(&f.cache())), (1, 2, before));
    // the flags read now: counted
    fs::remove_file(&flags).unwrap();
    let store = FlagStore::new(&f.pack, id.sizes.clone(), id.fingerprint.clone(), "bad").unwrap();
    store.mark_visited(RULE, 1).unwrap();
    store.mark_visited(RULE, 2).unwrap();
    assert_eq!(store.path(), flags);
    store.close();
    let kept = f.merged(1003, 10);
    f.agree(&kept, "flags that read now");
    assert_eq!(rows(&kept, &id)[RULE][..5], [2, 1, 0, 0, N - 2]);
    assert_eq!(kept.skipped.len(), 1);
    // flags that read at the last merge and read no more, their time as it
    // was: the count that meets them looks once more with every file opened
    // anew, and leaves them out as the exact report does
    let stamp = stamp_at(&flags);
    let whole = fs::read(&flags).unwrap();
    fs::write(&flags, b"not a database any more").unwrap();
    File::options()
        .write(true)
        .open(&flags)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_nanos(stamp))
        .unwrap();
    assert_eq!(stamp_at(&flags), stamp);
    poke(&bad, &id, 5, STATUS_WAIVED, None);
    for keep in [false, true] {
        let kept = if keep { f.merged(1004, 10) } else { f.counted(1004) };
        assert_eq!((rows(&kept, &id), kept.reviewers), f.exact(), "flags that read no more, kept: {keep}");
        assert!(kept.current && kept.stored == keep, "{:?}", kept.waiting);
        assert_eq!(rows(&kept, &id)[RULE][..5], [0, 2, 0, 0, N]);
        assert_eq!(kept.skipped.len(), 2, "{:?}", kept.skipped);
    }
    let before = state(&f.cache());
    f.agree(&f.merged(1005, 10), "... left out, once");
    assert_eq!(state(&f.cache()), before);
    // ... and counted again when they read
    fs::write(&flags, whole).unwrap();
    let kept = f.merged(1006, 10);
    f.agree(&kept, "flags that read again");
    assert_eq!(rows(&kept, &id)[RULE][..5], [2, 2, 0, 0, N - 2]);
}
