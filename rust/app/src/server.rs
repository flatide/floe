//! Explicit operator commands. TeeBox serving remains deferred; public demos
//! are separate from standalone authority and bind loopback behind a proxy.
mod demo;
use floe_app_core::{
    server::{Action, Config, Deployment},
    Error, Result,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{atomic::AtomicUsize, Arc},
};

const HELP: &str = "Usage: floe2-web server --check-config FILE
       floe2-web server --prepare-demo FILE [--jobs 1..12] [--lod]
       floe2-web server --demo FILE --proxy-key-file FILE --port PORT

--check-config validates the versioned policy without starting a server or
parsing/indexing/modifying a source. Roots must already exist, be absolute and
disjoint. Demo accepts only administrator-listed sample IDs. The check report
contains no credentials, paths or delegated user identities.

--prepare-demo explicitly builds/publishes immutable indexes for every configured
sample (default 4 jobs). This is an operator write, never a browser action.
--demo requires public_demo mode, ready revision sets and max_sessions <= 4.
It listens only on 127.0.0.1 behind the configured proxy (HTTPS by default).
Public demos may opt into unencrypted LAN testing with deployment.allow_insecure_http
set to true and an explicit http:// public_origin. Restrict proxy ingress to the
trusted test network; no network range or firewall is configured automatically.
The private key
file must be owned by this user, mode 0600/0400, with 64 lowercase hex digits.
No TLS/certificates/firewall changes are made. TeeBox serving remains deferred.
Standalone is unchanged. See docs/WEBUI_WEBSITE_DEMO.ko.md.";

pub enum Command {
    Help,
    Check(PathBuf),
    Prepare {
        config: PathBuf,
        jobs: usize,
        lod: bool,
    },
    Demo {
        config: PathBuf,
        key: PathBuf,
        port: u16,
    },
}
pub fn parse(args: &[String]) -> Result<Command> {
    if args
        .get(1)
        .is_some_and(|v| v == "--prepare-demo" || v == "--demo")
    {
        let prepare = args[1] == "--prepare-demo";
        let config = args
            .get(2)
            .filter(|p| !p.is_empty() && !p.starts_with('-'))
            .ok_or_else(|| Error::input("server mode requires a configuration file"))?;
        let (mut jobs, mut lod, mut key, mut port) = (None, false, None, None);
        let mut rest = args[3..].iter();
        while let Some(flag) = rest.next() {
            match flag.as_str() {
                "--lod" if prepare && !lod => lod = true,
                "--jobs" if prepare && jobs.is_none() => {
                    jobs = Some(
                        rest.next()
                            .and_then(|v| v.parse::<usize>().ok())
                            .filter(|v| (1..=12).contains(v))
                            .ok_or_else(|| Error::input("demo preparation jobs must be 1..12"))?,
                    );
                }
                "--proxy-key-file" if !prepare && key.is_none() => {
                    key = Some(PathBuf::from(
                        rest.next()
                            .filter(|v| !v.is_empty() && !v.starts_with('-'))
                            .ok_or_else(|| Error::input("missing proxy key file"))?,
                    ));
                }
                "--port" if !prepare && port.is_none() => {
                    port = Some(
                        rest.next()
                            .and_then(|v| v.parse::<u16>().ok())
                            .filter(|v| *v >= 1024)
                            .ok_or_else(|| Error::input("demo port must be 1024..65535"))?,
                    );
                }
                _ => return Err(Error::input("unsupported or repeated server option")),
            }
        }
        return if prepare {
            Ok(Command::Prepare {
                config: config.into(),
                jobs: jobs.unwrap_or(4),
                lod,
            })
        } else {
            Ok(Command::Demo {
                config: config.into(),
                key: key.ok_or_else(|| Error::input("--proxy-key-file is required"))?,
                port: port.ok_or_else(|| Error::input("--port is required"))?,
            })
        };
    }
    match &args[1..] {
        [help] if help == "--help" || help == "-h" => Ok(Command::Help),
        [flag, path] if flag == "--check-config" && !path.is_empty() && !path.starts_with('-') => {
            Ok(Command::Check(path.into()))
        }
        _ => Err(Error::input(
            "use server --help for policy check, demo preparation and demo serving",
        )),
    }
}
fn report(config: Config) -> Result<Value> {
    let validated = config.validate()?;
    let policy = validated.config();
    let actions: Vec<_> = [
        Action::View,
        Action::Browse,
        Action::RequestIndex,
        Action::ReadOwnReview,
        Action::WriteReview,
        Action::Share,
        Action::ExportGeometry,
        Action::PublishDefaults,
    ]
    .into_iter()
    .filter(|action| validated.allows(*action))
    .collect();
    let (samples, index) = match &policy.deployment {
        Deployment::TeeBox { index, .. } => (0, Some(index)),
        Deployment::PublicDemo { samples, .. } => (samples.len(), None),
    };
    Ok(json!({"schema":1, "check":"server-policy", "valid":true,
        "runtime_ready":false, "mode":validated.mode(),
        "insecure_http_test":validated.http_test(),
        "max_sessions":policy.max_sessions, "demo_samples":samples,
        "allowed_actions":actions, "index_policy":index,
        "checks":["schema", "public_origin_transport_policy", "existing_disjoint_roots", "demo_source_scope"],
        "not_checked":["authentication", "tls", "filesystem_permissions", "ready_indexes", "runtime_limits"]}))
}
pub fn run(command: Command, stop: &Arc<AtomicUsize>) -> Result<i32> {
    match command {
        Command::Help => println!("{HELP}"),
        Command::Check(path) => println!(
            "{}",
            serde_json::to_string_pretty(&report(Config::read(&path)?)?)
                .map_err(|_| Error::input("cannot encode server policy report"))?
        ),
        Command::Prepare { config, jobs, lod } => demo::prepare(&config, jobs, lod, stop)?,
        Command::Demo { config, key, port } => demo::serve(&config, &key, port, stop)?,
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).into()).collect()
    }
    #[test]
    fn preflight_never_falls_through_to_local_view() {
        assert!(matches!(
            parse(&args(&["server", "--help"])).unwrap(),
            Command::Help
        ));
        assert!(matches!(
            parse(&args(&[
                "server",
                "--check-config",
                "/not/read/at/parse.json"
            ]))
            .unwrap(),
            Command::Check(_)
        ));
        for values in [
            vec!["server"],
            vec!["server", "--listen", "0.0.0.0:80"],
            vec!["server", "--check-config"],
            vec!["server", "--check-config", "--help"],
            vec!["server", "--check-config", "one", "two"],
            vec!["server", "--help", "--check-config", "one"],
            vec!["server", "--demo", "one"],
            vec!["server", "--prepare-demo", "one", "--jobs", "13"],
            vec![
                "server",
                "--prepare-demo",
                "one",
                "--jobs",
                "1",
                "--jobs",
                "2",
            ],
            vec![
                "server",
                "--demo",
                "one",
                "--proxy-key-file",
                "key",
                "--port",
                "0",
            ],
            vec![
                "server",
                "--demo",
                "one",
                "--proxy-key-file",
                "key",
                "--port",
                "58080",
                "--listen",
                "0.0.0.0",
            ],
        ] {
            assert!(parse(&args(&values)).is_err());
        }
        assert!(matches!(
            parse(&args(&[
                "server",
                "--prepare-demo",
                "one",
                "--jobs",
                "2",
                "--lod"
            ]))
            .unwrap(),
            Command::Prepare {
                jobs: 2,
                lod: true,
                ..
            }
        ));
        assert!(matches!(
            parse(&args(&[
                "server",
                "--demo",
                "one",
                "--proxy-key-file",
                "key",
                "--port",
                "58080"
            ]))
            .unwrap(),
            Command::Demo { port: 58080, .. }
        ));
    }
}
