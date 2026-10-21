//! Serves a folder with the share server, for working on the share page:
//! `FIVEFILES_SHARE_PAGE=src/share/page.html cargo run --example share_demo -- <folder> [--upload] [--pin 1234]`

use fivefiles_lib::share::{ShareOptions, Shares};
use fivefiles_lib::vfs::{DynProvider, local::LocalFs};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().expect("usage: share_demo <folder> [--upload] [--pin 1234]").clone();
    let upload = args.iter().any(|a| a == "--upload");
    let pin = args.iter().position(|a| a == "--pin").and_then(|i| args.get(i + 1)).cloned();
    let path = std::fs::canonicalize(&dir).expect("folder").to_string_lossy().into_owned();
    let name = std::path::Path::new(&path).file_name().unwrap().to_string_lossy().into_owned();
    let shares = Shares::new(std::env::temp_dir().join("5files-share-demo-thumbs"), Arc::new(|| {}));
    let p: DynProvider = Arc::new(LocalFs);
    let v = shares.start("local".into(), path, name, p, ShareOptions { allow_upload: upload, pin, ..Default::default() }).await.unwrap();
    println!("{}", v.url);
    println!("http://127.0.0.1:{}/s/{}/", shares.port(), v.id);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        if let Some(v) = shares.views().first() {
            for a in v.activity.iter().take(1) {
                eprintln!("[{}] {} {}", a.who, a.action, a.name);
            }
        }
    }
}
