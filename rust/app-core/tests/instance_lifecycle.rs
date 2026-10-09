//! Real subprocess ownership/recovery. This executable is its own fixture;
//! no Python, shell, X server, browser or layout/index writes are involved.
use floe_app_core::instance::{Claim, Endpoint, Key, Outcome, Reject};
use serde_json::json;
use std::{
    fs::{self, DirBuilder},
    io::{BufRead, BufReader, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, MetadataExt},
            net::UnixStream,
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
const BUILD: &str = "fixture-v1";
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "--owner") {
        fixture(Path::new(&args[2]), &args[3]);
        return;
    }
    if args.get(1).is_some_and(|s| s == "--preexec-window") {
        owner_release_during_a_childs_preexec_window();
        return;
    }
    roundtrip_and_crash();
    owner_race();
    lock_is_not_inherited_by_exec();
    owner_release_during_a_childs_preexec_window();
    println!("INSTANCE LIFECYCLE: ALL OK (native peer identity, cross-process lease, concurrent claims, same-intent replay, build refusal, SIGKILL recovery, changed epoch, inode-safe cleanup, pre-exec lease release, close-on-exec)");
}
fn endpoint(path: &Path, display: &str) -> Endpoint {
    Endpoint::in_directory(path, &Key::new("floe2-web", Some(display)).unwrap()).unwrap()
}
fn fixture(path: &Path, display: &str) {
    if display == "hold" {
        println!("hold");
        std::io::stdout().flush().unwrap();
        while !path.join("stop").exists() {
            thread::sleep(Duration::from_millis(5));
        }
        return;
    }
    if path.join("barrier").exists() {
        let start = Instant::now();
        while !path.join("go").exists() {
            assert!(start.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(5));
        }
    }
    let endpoint = endpoint(path, display);
    let owner = match endpoint.claim(BUILD).unwrap() {
        Claim::Running(_) => {
            println!("running");
            return;
        }
        Claim::Owner(owner) => owner,
    };
    println!("owner");
    std::io::stdout().flush().unwrap();
    let stop = Arc::new(AtomicUsize::new(0));
    let watched = Arc::clone(&stop);
    let path = path.to_owned();
    let watcher = thread::spawn(move || {
        while watched.load(Ordering::Relaxed) == 0 {
            if path.join("stop").exists() {
                watched.store(1, Ordering::Relaxed);
            }
            thread::sleep(Duration::from_millis(5));
        }
    });
    let mut calls = 0;
    owner
        .serve(&stop, |request, _| {
            calls += 1;
            Ok(json!({"calls":calls,"request":request}))
        })
        .unwrap();
    stop.store(1, Ordering::Relaxed);
    watcher.join().unwrap();
}
struct Temp(PathBuf);
impl Temp {
    fn new(name: &str) -> Self {
        let path = Path::new("/tmp").join(format!("fi-native-{}-{name}", std::process::id()));
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
    fn socket_and_lock(&self) -> (PathBuf, PathBuf) {
        let directory = fs::read_dir(&self.0)
            .unwrap()
            .map(|p| p.unwrap().path())
            .find(|p| p.is_dir())
            .unwrap();
        let paths = fs::read_dir(directory)
            .unwrap()
            .map(|p| p.unwrap().path())
            .collect::<Vec<_>>();
        (
            paths
                .iter()
                .find(|p| p.extension().is_some_and(|s| s == "sock"))
                .unwrap()
                .clone(),
            paths
                .iter()
                .find(|p| p.extension().is_some_and(|s| s == "lock"))
                .unwrap()
                .clone(),
        )
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
struct Fixture(Child);
impl Fixture {
    fn spawn(temp: &Temp, display: &str) -> Self {
        Self(
            Command::new(std::env::current_exe().unwrap())
                .arg("--owner")
                .arg(&temp.0)
                .arg(display)
                .env("PATH", "")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        )
    }
    fn state(&mut self) -> String {
        // Child stdout has only one small line. The bounded channel reader
        // prevents a failed startup from hanging the entire test battery.
        let stdout = self.0.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut text = String::new();
            BufReader::new(stdout).read_line(&mut text).unwrap();
            let _ = tx.send(text);
        });
        rx.recv_timeout(Duration::from_secs(5))
            .unwrap()
            .trim()
            .to_owned()
    }
    fn wait(&mut self, success: bool) {
        let start = Instant::now();
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert_eq!(status.success(), success);
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "fixture did not exit"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
fn roundtrip_and_crash() {
    let temp = Temp::new("restart");
    let endpoint = endpoint(&temp.0, ":8.0");
    let stop = AtomicUsize::new(0);
    let mut child = Fixture::spawn(&temp, ":8");
    assert_eq!(child.state(), "owner");
    assert!(matches!(endpoint.claim(BUILD).unwrap(), Claim::Running(_)));
    let client = endpoint.connect(BUILD, &stop).unwrap();
    let intent = client
        .intent(json!({"source":"한 글.oas","goto":[1,2]}))
        .unwrap();
    let result = client.submit(&intent, &stop).unwrap();
    assert!(matches!(&result,Outcome::Handled{reply} if reply["calls"]==1));
    assert_eq!(
        endpoint
            .connect(BUILD, &stop)
            .unwrap()
            .submit(&intent, &stop)
            .unwrap(),
        result
    );
    assert!(endpoint.connect("wrong-build", &stop).is_err());
    let (socket, lock) = temp.socket_and_lock();
    let before = fs::metadata(&lock).unwrap().ino();
    child.0.kill().unwrap();
    child.wait(false);
    assert!(socket.exists());
    let mut next = Fixture::spawn(&temp, ":8.1");
    assert_eq!(next.state(), "owner");
    assert_eq!(fs::metadata(&lock).unwrap().ino(), before);
    assert_eq!(
        endpoint
            .connect(BUILD, &stop)
            .unwrap()
            .submit(&intent, &stop)
            .unwrap(),
        Outcome::Rejected {
            code: Reject::OwnerChanged
        }
    );
    let client = endpoint.connect(BUILD, &stop).unwrap();
    let request = client.intent(json!({"source":"new"})).unwrap();
    assert!(
        matches!(client.submit(&request,&stop).unwrap(),Outcome::Handled{reply} if reply["calls"]==1)
    );
    fs::write(temp.0.join("stop"), b"stop").unwrap();
    next.wait(true);
    assert!(!socket.exists());
    assert_eq!(fs::metadata(&lock).unwrap().ino(), before);
    drop(endpoint.claim(BUILD).unwrap());
    assert!(!socket.exists());
}
fn owner_race() {
    let temp = Temp::new("race");
    fs::write(temp.0.join("barrier"), b"").unwrap();
    let mut a = Fixture::spawn(&temp, ":9");
    let mut b = Fixture::spawn(&temp, ":9.0");
    fs::write(temp.0.join("go"), b"").unwrap();
    let mut states = [a.state(), b.state()];
    states.sort();
    assert_eq!(states, ["owner", "running"]);
    let stop = AtomicUsize::new(0);
    let client = endpoint(&temp.0, ":9").connect(BUILD, &stop).unwrap();
    let request = client.intent(json!({"race":true})).unwrap();
    assert!(
        matches!(client.submit(&request,&stop).unwrap(),Outcome::Handled{reply} if reply["calls"]==1)
    );
    fs::write(temp.0.join("stop"), b"stop").unwrap();
    a.wait(true);
    b.wait(true);
}
fn lock_is_not_inherited_by_exec() {
    let temp = Temp::new("exec");
    let endpoint = endpoint(&temp.0, ":10");
    let Claim::Owner(owner) = endpoint.claim(BUILD).unwrap() else {
        panic!("owned")
    };
    let mut child = Fixture::spawn(&temp, "hold");
    assert_eq!(child.state(), "hold");
    drop(owner);
    // The child remains alive but must not retain the parent's flock fd.
    let Claim::Owner(next) = endpoint.claim(BUILD).unwrap() else {
        panic!("lock inherited by exec")
    };
    drop(next);
    fs::write(temp.0.join("stop"), b"stop").unwrap();
    child.wait(true);
}

fn owner_release_during_a_childs_preexec_window() {
    let temp = Temp::new("preexec");
    let endpoint = endpoint(&temp.0, ":11");
    let Claim::Owner(owner) = endpoint.claim(BUILD).unwrap() else {
        panic!("owned")
    };
    let (_, lock) = temp.socket_and_lock();
    let inode = fs::metadata(&lock).unwrap().ino();
    let (mut ready_rx, ready_tx) = UnixStream::pair().unwrap();
    let (mut release_tx, release_rx) = UnixStream::pair().unwrap();
    for socket in [&ready_rx, &ready_tx, &release_tx, &release_rx] {
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
    }
    let exe = std::env::current_exe().unwrap();
    let path = temp.0.clone();
    let spawn = thread::spawn(move || {
        let mut command = Command::new(exe);
        command
            .arg("--owner")
            .arg(path)
            .arg("hold")
            .env("PATH", "")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        // SAFETY: the child hook only calls read/write on valid, pre-created
        // descriptors with kernel timeouts. No allocation, Rust locks or I/O
        // formatting occurs between fork and exec. CLOEXEC is left untouched.
        unsafe {
            command.pre_exec(move || {
                let mut byte = 1u8;
                for (fd, send) in [
                    (ready_tx.as_raw_fd(), true),
                    (release_rx.as_raw_fd(), false),
                ] {
                    loop {
                        let n = if send {
                            libc::write(fd, (&byte as *const u8).cast(), 1)
                        } else {
                            libc::read(fd, (&mut byte as *mut u8).cast(), 1)
                        };
                        if n == 1 {
                            break;
                        }
                        let error = if n == 0 {
                            std::io::Error::from_raw_os_error(libc::EIO)
                        } else {
                            std::io::Error::last_os_error()
                        };
                        if error.raw_os_error() != Some(libc::EINTR) {
                            return Err(error);
                        }
                    }
                }
                Ok(())
            });
        }
        command.spawn()
    });
    let mut ready = [0u8];
    let readiness = ready_rx.read_exact(&mut ready);
    // The child now holds inherited lock/listener descriptors but has not
    // executed its program. Do not release it until AFTER probing reclaim.
    drop(owner);
    let reclaimed = if readiness.is_ok() {
        Some(endpoint.claim(BUILD))
    } else {
        None
    };
    let released = release_tx.write_all(&[1]);
    let child = spawn.join().unwrap();
    // Reap the fixture before assertions, including the expected pre-fix failure.
    let mut child = Fixture(child.unwrap());
    let state = child.state();
    fs::write(temp.0.join("stop"), b"stop").unwrap();
    child.wait(true);
    readiness.unwrap();
    released.unwrap();
    assert_eq!(state, "hold");
    let Claim::Owner(next) = reclaimed.unwrap().unwrap() else {
        panic!("owner lease survived its drop during another thread's pre-exec window");
    };
    assert_eq!(fs::metadata(&lock).unwrap().ino(), inode);
    assert!(matches!(endpoint.claim(BUILD).unwrap(), Claim::Running(_)));
    drop(next);
    assert!(matches!(endpoint.claim(BUILD).unwrap(), Claim::Owner(_)));
}
