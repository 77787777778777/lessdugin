// SPDX-License-Identifier: AGPL-3.0-or-later
//! `devnet_services --pds <url> --json <file>`: the node devnet's CORE+ services
//! ([`lessdugin_testkit::devnet_services`]) for the Playwright parity scenario
//! (`web/e2e/devnet/scenario.spec.ts`). Writes `{jetstream, constellation, relay, control}`
//! (loopback URLs; `control` serves `/_devnet/log` and `/_devnet/record`) to the file once the
//! listeners are bound, then runs until it is killed.

use lessdugin_testkit::devnet_services::DevnetServices;

fn arg(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(pds), Some(json)) = (arg(&args, "--pds"), arg(&args, "--json")) else {
        eprintln!("usage: devnet_services --pds <url> --json <file>");
        std::process::exit(2);
    };
    let services = DevnetServices::start(&pds);
    let state = serde_json::json!({
        "jetstream": services.jetstream_url(),
        "constellation": services.constellation_url(),
        "relay": services.relay_url(),
        "control": services.constellation_url(),
    });
    // written whole, then renamed, so a reader never sees half a file
    let tmp = format!("{json}.tmp");
    std::fs::write(&tmp, state.to_string()).expect("write the services JSON");
    std::fs::rename(&tmp, &json).expect("publish the services JSON");
    eprintln!("devnet services: {state}");
    loop {
        std::thread::park();
    }
}
