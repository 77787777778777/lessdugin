// SPDX-License-Identifier: AGPL-3.0-or-later
//! Render LFM/1 from stdin and print the outputs (a development aid).
//!
//! `cargo run -p lessdugin-markdown --example render [--json|--print|--color] < doc.md`

use std::io::Read;

use lessdugin_markdown::printer::{PrintOpts, print_doc};
use lessdugin_markdown::{RenderOpts, render};

fn main() {
    let mut src = String::new();
    std::io::stdin().read_to_string(&mut src).expect("stdin");
    let r = render(&src, &[], &RenderOpts::ALL);
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "--json" => println!("{}", serde_json::to_string_pretty(&r).expect("json")),
        "--print" | "--color" => {
            let color = mode == "--color";
            let opts = PrintOpts {
                color,
                hyperlinks: color,
                reveal_spoilers: false,
                width: Some(60),
            };
            print!("{}", print_doc(&r.styled, &opts));
        }
        _ => {
            print!("{}", r.html);
            println!("--- P: {:?}", r.text);
            println!("--- flags: {:?} anchors: {:?}", r.flags, r.anchors);
        }
    }
}
