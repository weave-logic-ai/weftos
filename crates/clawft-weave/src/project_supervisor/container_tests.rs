use super::*;
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Mutex;

const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PROJECT: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";
fn cfg() -> OperatorConfig {
    OperatorConfig {
        engine: "docker".into(),
        image: format!("weftos@sha256:{}", "b".repeat(64)),
    }
}
fn mounts() -> Mounts {
    Mounts {
        project: "/work/p".into(),
        runtime: "/run/guest".into(),
        trust: "/run/trust".into(),
        link: "/run/link".into(),
        guest_runtime: format!("/weftos/run/{PROJECT}"),
        supervisor_id: "owner-key".into(),
    }
}
fn inspected(id: &str, running: bool) -> String {
    let user = format!(
        "{}:{}",
        nix::unistd::geteuid().as_raw(),
        nix::unistd::getegid().as_raw()
    );
    serde_json::json!([{ "Id": id, "State": {"Running": running, "Pid": if running {4242} else {0}, "ExitCode": 0},
        "Config": {"Image": cfg().image, "Entrypoint": [GUEST_EXECUTABLE], "User": user, "Labels": {"weftos.project": PROJECT, "weftos.supervisor": "owner-key"}},
        "HostConfig": {"ReadonlyRootfs":true,"Privileged":false,"CapAdd":null,"CapDrop":["ALL"],
            "SecurityOpt":["no-new-privileges"],"PidsLimit":64,"NetworkMode":"none"},
        "NetworkSettings": {"Networks":{}},
        "Mounts": [
            {"Source":"/work/p","Destination":GUEST_PROJECT,"RW":true},
            {"Source":"/run/guest","Destination":format!("/weftos/run/{PROJECT}"),"RW":true},
            {"Source":"/run/trust","Destination":GUEST_TRUST,"RW":false},
            {"Source":"/run/link","Destination":GUEST_LINK,"RW":false}
        ] }]).to_string()
}
#[test]
fn flags_pin_isolation_and_parent_mounts() {
    let args = create_args(&cfg(), PROJECT, &mounts(), 1000, 1000)
        .unwrap()
        .join(" ");
    for required in [
        "--read-only",
        "--cap-drop ALL",
        "no-new-privileges",
        "--pids-limit 64",
        "--network none",
        "--user 1000:1000",
        "--entrypoint /usr/local/bin/weaver",
        "/run/trust,dst=/weftos/trust,readonly",
        "/run/guest,dst=/weftos/run/",
    ] {
        assert!(args.contains(required), "missing {required}: {args}");
    }
    assert!(!args.contains("--privileged"));
}
#[test]
fn no_guest_mount_descends_from_or_hides_the_executable() {
    for engine in ["docker", "podman"] {
        let mut config = cfg();
        config.engine = engine.into();
        let args = create_args(&config, PROJECT, &mounts(), 1000, 1000).unwrap();
        let executable = args.windows(2).find(|w| w[0] == "--entrypoint").unwrap();
        assert_eq!(executable[1], "/usr/local/bin/weaver");
        let binary = Path::new(&executable[1]);
        let targets: Vec<&str> = args.windows(2)
            .filter(|w| w[0] == "--mount")
            .map(|w| w[1].split(',').find_map(|part| part.strip_prefix("dst=")).unwrap())
            .collect();
        assert_eq!(targets.len(), 4);
        for target in &targets {
            let mount = Path::new(target);
            assert!(!mount.starts_with(binary), "mount {target} descends from executable {}", binary.display());
            assert!(!binary.starts_with(mount), "mount {target} hides executable {}", binary.display());
        }
        // The old executable really conflicts with these same mount targets.
        // Changing only the entrypoint back to /weftos makes this test fail.
        assert!(targets.iter().all(|target| Path::new(target).starts_with("/weftos")));
        assert_eq!(&args[args.len() - crate::project_supervisor::child::child_args(PROJECT).len()..],
                   crate::project_supervisor::child::child_args(PROJECT).as_slice());
    }
}

#[test]
fn runtime_mount_must_not_cover_or_descend_from_weaver() {
    for target in ["/usr/local/bin/weaver", "/usr/local/bin/weaver/run", "/usr/local/bin", "/", "relative/run", "/case/../usr/local/bin"] {
        let mut m = mounts();
        m.guest_runtime = target.into();
        assert!(create_args(&cfg(), PROJECT, &m, 1000, 1000).is_err(), "accepted {target}");
    }
}

#[test]
fn forged_id_label_or_mount_is_refused() {
    assert!(parse_inspect(&inspected(ID, true), ID, PROJECT, &mounts(), &cfg().image).is_ok());
    assert!(
        parse_inspect(
            &inspected(&"c".repeat(64), true),
            ID,
            PROJECT,
            &mounts(),
            &cfg().image
        )
        .is_err()
    );
    assert!(parse_inspect(&inspected(ID, true), ID, "another", &mounts(), &cfg().image).is_err());
    assert!(
        parse_inspect(
            &inspected(ID, true),
            ID,
            PROJECT,
            &mounts(),
            "other@sha256:deadbeef"
        )
        .is_err()
    );
    let mut wrong = mounts();
    wrong.trust = "/attacker/trust".into();
    assert!(parse_inspect(&inspected(ID, true), ID, PROJECT, &wrong, &cfg().image).is_err());
}
#[test]
fn weakened_isolation_and_extra_mount_are_refused() {
    let original: serde_json::Value = serde_json::from_str(&inspected(ID, true)).unwrap();
    for (pointer, value) in [
        ("/0/HostConfig/ReadonlyRootfs", serde_json::json!(false)),
        ("/0/HostConfig/Privileged", serde_json::json!(true)),
        ("/0/HostConfig/CapDrop", serde_json::json!([])),
        ("/0/HostConfig/CapAdd", serde_json::json!(["SYS_ADMIN"])),
        ("/0/HostConfig/SecurityOpt", serde_json::json!([])),
        ("/0/HostConfig/PidsLimit", serde_json::json!(0)),
        ("/0/HostConfig/NetworkMode", serde_json::json!("bridge")),
        (
            "/0/NetworkSettings/Networks",
            serde_json::json!({"bridge":{}}),
        ),
        ("/0/Config/User", serde_json::json!("999999:999999")),
        ("/0/Config/Entrypoint", serde_json::json!(["/weftos"])),
        ("/0/Config/Entrypoint", serde_json::json!(["/usr/local/bin/weft"])),
        ("/0/Config/Entrypoint", serde_json::json!([GUEST_EXECUTABLE, "--version"])),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(
            parse_inspect(&changed.to_string(), ID, PROJECT, &mounts(), &cfg().image).is_err(),
            "accepted {pointer}"
        );
    }
    let mut extra = original;
    extra
        .pointer_mut("/0/Mounts")
        .unwrap()
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "Source":"/secrets", "Destination":"/leak", "RW":true
        }));
    assert!(parse_inspect(&extra.to_string(), ID, PROJECT, &mounts(), &cfg().image).is_err());
}
struct Fake {
    calls: Mutex<Vec<String>>,
    replies: Mutex<VecDeque<CmdOutput>>,
}
#[async_trait]
impl CommandRunner for Fake {
    async fn run(
        &self,
        _program: &str,
        args: &[String],
        _timeout: Duration,
        _limit: usize,
    ) -> Result<CmdOutput, clawft_kernel::workload_runtime::RuntimeError> {
        self.calls.lock().unwrap().push(args.join(" "));
        Ok(self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected engine call"))
    }
}
fn ok(s: String) -> CmdOutput {
    CmdOutput {
        status: Some(0),
        stdout: s,
        ..CmdOutput::default()
    }
}
#[tokio::test]
async fn create_start_stop_reinspect_immutable_id() {
    let fake = Arc::new(Fake {
        calls: Mutex::new(Vec::new()),
        replies: Mutex::new(VecDeque::from([
            ok(ID.into()),
            ok(inspected(ID, false)), // start preflight
            ok(String::new()),
            ok(inspected(ID, true)), // start
            ok(inspected(ID, true)),
            ok(String::new()),
            ok(inspected(ID, false)), // stop
        ])),
    });
    let client = EngineClient {
        cfg: cfg(),
        runner: fake.clone(),
    };
    assert_eq!(
        client
            .create_unverified(
                PROJECT,
                &mounts(),
                nix::unistd::geteuid().as_raw(),
                nix::unistd::getegid().as_raw()
            )
            .await
            .unwrap(),
        ID
    );
    assert_eq!(
        client.start(ID, PROJECT, &mounts()).await.unwrap().host_pid,
        Some(4242)
    );
    client
        .stop(ID, PROJECT, &mounts(), Duration::from_secs(1))
        .await
        .unwrap();
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 7);
    assert!(
        calls
            .iter()
            .filter(|c| c == &&format!("inspect {ID}"))
            .count()
            >= 3
    );
    assert!(calls[5].starts_with(&format!("stop -t 1 {ID}")));
}

#[tokio::test]
async fn stale_id_never_reaches_a_destructive_engine_call() {
    let fake = Arc::new(Fake {
        calls: Mutex::new(Vec::new()),
        replies: Mutex::new(VecDeque::from([ok(inspected(&"c".repeat(64), true))])),
    });
    let client = EngineClient {
        cfg: cfg(),
        runner: fake.clone(),
    };
    assert!(
        client
            .stop(ID, PROJECT, &mounts(), Duration::from_secs(1))
            .await
            .is_err()
    );
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.as_slice(), &[format!("inspect {ID}")]);
}

#[tokio::test]
async fn leftover_name_recovers_only_a_verified_immutable_id() {
    let fake = Arc::new(Fake {
        calls: Mutex::new(Vec::new()),
        replies: Mutex::new(VecDeque::from([ok(inspected(ID, false))])),
    });
    let client = EngineClient {
        cfg: cfg(),
        runner: fake.clone(),
    };
    assert_eq!(
        client.inspect_named(PROJECT, &mounts()).await.unwrap().id,
        ID
    );
    assert_eq!(
        fake.calls.lock().unwrap().as_slice(),
        &[format!("inspect {}", name(PROJECT))]
    );

    let fake = Arc::new(Fake {
        calls: Mutex::new(Vec::new()),
        replies: Mutex::new(VecDeque::from([ok(inspected(ID, false))])),
    });
    let client = EngineClient {
        cfg: cfg(),
        runner: fake.clone(),
    };
    assert!(client.inspect_named("foreign", &mounts()).await.is_err());
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn create_returns_id_before_any_inspect() {
    let fake = Arc::new(Fake {
        calls: Mutex::new(Vec::new()),
        replies: Mutex::new(VecDeque::from([ok(ID.into())])),
    });
    let client = EngineClient {
        cfg: cfg(),
        runner: fake.clone(),
    };
    assert_eq!(
        client
            .create_unverified(PROJECT, &mounts(), 1000, 1000)
            .await
            .unwrap(),
        ID
    );
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("create --pull=never "));
}

#[tokio::test]
async fn forced_stop_preserves_the_id_for_verified_restart_cleanup() {
    let fake = Arc::new(Fake {
        calls: Mutex::new(Vec::new()),
        replies: Mutex::new(VecDeque::from([
            ok(inspected(ID, true)),
            CmdOutput {
                status: Some(1),
                stderr: "grace elapsed".into(),
                ..CmdOutput::default()
            },
            ok(inspected(ID, true)),
            ok(inspected(ID, true)),
            ok(String::new()),
            ok(inspected(ID, false)),
            ok(inspected(ID, false)),
            ok(inspected(ID, false)),
            ok(String::new()),
            ok(ID.into()),
        ])),
    });
    let client = EngineClient {
        cfg: cfg(),
        runner: fake.clone(),
    };
    client
        .stop(ID, PROJECT, &mounts(), Duration::from_secs(1))
        .await
        .unwrap();
    client.remove_exited(ID, PROJECT, &mounts()).await.unwrap();
    assert_eq!(
        client
            .create_unverified(PROJECT, &mounts(), 1000, 1000)
            .await
            .unwrap(),
        ID
    );
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 10);
    assert_eq!(
        &calls[..9],
        &[
            format!("inspect {ID}"),
            format!("stop -t 1 {ID}"),
            format!("inspect {ID}"),
            format!("inspect {ID}"),
            format!("kill {ID}"),
            format!("inspect {ID}"),
            format!("inspect {ID}"),
            format!("inspect {ID}"),
            format!("rm {ID}"),
        ]
    );
    assert!(calls.last().unwrap().starts_with("create --pull=never "));
}
