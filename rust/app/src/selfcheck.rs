//! Local deployment diagnostic, not a browser capability or a sign-off gate.
use floe_app_core::{
    browser, check_cancelled,
    native::{Discovery, Indexer, INDEX_VERSION},
    Error, Result,
};
use floe_worker_client::{Config, WorkerClient, EXPECTED_RENDERD_VERSION};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{atomic::AtomicUsize, Arc},
    time::Duration,
};

const HELP: &str = "Usage: floe2-web selfcheck [--adjacent] [--metadata-only]

Print JSON build identity and local native-runtime checks. Exit 0 means only
the reported checks passed; exit 1 means a required runtime check failed.
--adjacent      Check only floe-index/renderd beside this executable; ignore
                environment overrides, development checkout and PATH discovery.
--metadata-only Print this binary's metadata without discovering/spawning tools.
Default         Use the same explicit overrides/discovery as normal commands.

No source file is opened/indexed/rendered, no listener or browser is started.
The renderd handshake uses and removes its own temporary working directory.
Firefox discovery does not execute Firefox, validate its version/features or
prove desktop readiness. Missing Firefox is informational (--no-open is valid).
This is not an ELF/glibc audit, pixel test, or Firefox/ETX/NFS acceptance gate.";

#[derive(Default)]
pub struct Options {
    adjacent: bool,
    metadata_only: bool,
    help: bool,
}
pub fn parse(args: &[String]) -> Result<Options> {
    let mut o = Options::default();
    for arg in &args[1..] {
        match arg.as_str() {
            "--adjacent" if !o.adjacent => o.adjacent = true,
            "--metadata-only" if !o.metadata_only => o.metadata_only = true,
            "--help" | "-h" if !o.help => o.help = true,
            _ => {
                return Err(Error::input(format!(
                    "unknown/repeated selfcheck option: {arg}"
                )))
            }
        }
    }
    Ok(o)
}
pub fn metadata() -> Value {
    json!({"product":"floe2-web","app_version":env!("CARGO_PKG_VERSION"),
        "source_revision":env!("FLOE_APP_REVISION"),"target":env!("FLOE_APP_TARGET"),
        "web_bundle":floe_web::transport::BUNDLE,"index_version":INDEX_VERSION,
        "renderd_version":EXPECTED_RENDERD_VERSION,"python_runtime":false,
        "desktop_acceptance":"unverified"})
}
fn discovery(adjacent: bool, renderer: bool) -> Result<Discovery> {
    if !adjacent {
        return if renderer {
            Discovery::renderer()
        } else {
            Discovery::local()
        };
    }
    let executable = std::env::current_exe()?;
    let parent = executable
        .parent()
        .ok_or_else(|| Error::input("executable has no parent"))?;
    Ok(Discovery {
        override_path: Some(parent.join(if renderer {
            "floe-renderd"
        } else {
            "floe-index"
        })),
        development_root: None,
        executable,
        search_path: None,
    })
}
fn renderer(path: &Path, cancelled: &Arc<AtomicUsize>) -> Result<()> {
    let mut config = Config::new(path);
    config.ready_timeout = Duration::from_secs(5);
    config.shutdown_requested = Some(Arc::clone(cancelled));
    let mut worker = WorkerClient::spawn(config)?;
    let directory = worker.work_dir().to_owned();
    worker.close()?;
    if worker.pid().is_some() || directory.exists() {
        return Err(Error::input("renderd selfcheck cleanup incomplete"));
    }
    Ok(())
}
pub fn run(o: Options, cancelled: &Arc<AtomicUsize>) -> Result<i32> {
    if o.help {
        println!("{HELP}");
        return Ok(0);
    }
    let mut result = metadata();
    if o.metadata_only {
        result["checks"] = json!([]);
        result["runtime_checked"] = json!(false);
    } else {
        let mut checks = Vec::new();
        for is_renderer in [false, true] {
            check_cancelled(cancelled)?;
            let name = if is_renderer {
                "renderd_handshake_and_shutdown"
            } else {
                "indexer_version"
            };
            let mut path = None;
            let checked = (|| {
                let d = discovery(o.adjacent, is_renderer)?;
                if is_renderer {
                    let p = d.renderd_path()?;
                    path = Some(p.clone());
                    renderer(&p, cancelled)
                } else {
                    let indexer = Indexer::discover(&d)?;
                    path = Some(indexer.path().to_owned());
                    indexer.verify(cancelled)
                }
            })();
            check_cancelled(cancelled)?;
            checks.push(json!({"name":name,"ok":checked.is_ok(),"path":path,"error":checked.err().map(|e| e.to_string())}));
        }
        let firefox = match browser::discover(None) {
            Ok(path) => json!({"available":true,"path":path,"executed":false}),
            Err(e) => json!({"available":false,"error":e.to_string(),"executed":false}),
        };
        result["firefox_discovery"] = firefox;
        result["scope"] = json!(if o.adjacent {
            "adjacent_only"
        } else {
            "normal_discovery"
        });
        result["runtime_checked"] = json!(true);
        result["ok"] = json!(checks.iter().all(|c| c["ok"] == true));
        result["checks"] = json!(checks);
    }
    let code = i32::from(result["ok"] == false);
    println!("{result:#}");
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_is_explicit_not_desktop_acceptance() {
        let m = metadata();
        assert_eq!(m["python_runtime"], false);
        assert_eq!(m["desktop_acceptance"], "unverified");
        assert_eq!(m["web_bundle"].as_str().unwrap().len(), 40);
        assert!(!m["target"].as_str().unwrap().is_empty());
    }
    #[test]
    fn parser_rejects_sources_and_unknown_or_repeated_flags() {
        for args in [
            vec!["selfcheck", "design.oas"],
            vec!["selfcheck", "--force"],
            vec!["selfcheck", "--adjacent", "--adjacent"],
        ] {
            assert!(parse(&args.into_iter().map(String::from).collect::<Vec<_>>()).is_err());
        }
    }
}
