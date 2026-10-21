//! The folder-sharing web server end to end: real sockets, real HTTP client.

use fivefiles_lib::share::{ShareOptions, Shares};
use fivefiles_lib::vfs::{DynProvider, local::LocalFs};
use std::sync::Arc;

struct Fixture {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    outside: std::path::PathBuf,
    shares: Arc<Shares>,
    base: String,
    id: String,
    http: reqwest::Client,
}

async fn setup(opts: ShareOptions) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Shared Photos");
    let outside = dir.path().join("private");
    std::fs::create_dir_all(root.join("trip")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(root.join("notes.txt"), "0123456789abcdef").unwrap();
    std::fs::write(root.join("page.html"), "<script>alert(1)</script>").unwrap();
    std::fs::write(root.join(".secret"), "hidden").unwrap();
    std::fs::write(root.join("trip").join("día 1.bin"), vec![3u8; 300_000]).unwrap();
    std::fs::write(outside.join("passwords.txt"), "nope").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();

    let shares = Shares::new(dir.path().join("thumbs"), Arc::new(|| {}));
    let p: DynProvider = Arc::new(LocalFs);
    let v = shares.start("local".into(), root.to_string_lossy().into(), "Shared Photos".into(), p, opts).await.unwrap();
    let base = format!("http://127.0.0.1:{}/s/{}", shares.port(), v.id);
    assert!(v.url.ends_with(&format!("/s/{}/", v.id)));
    Fixture { _dir: dir, root, outside, shares, base, id: v.id, http: reqwest::Client::new() }
}

#[tokio::test]
async fn browse_download_and_stay_inside_the_folder() {
    let f = setup(ShareOptions::default()).await;
    let get = |u: String| f.http.get(u).send();

    // The page itself, and a redirect to the canonical trailing-slash URL.
    let r = get(format!("{}/", f.base)).await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-type"].to_str().unwrap().starts_with("text/html"));
    let r = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap().get(&f.base).send().await.unwrap();
    assert_eq!(r.status(), 308);

    // Listing: hidden files and links leading outside are left out.
    let list: serde_json::Value = get(format!("{}/api/list?p=", f.base)).await.unwrap().json().await.unwrap();
    let names: Vec<&str> = list["items"].as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"notes.txt") && names.contains(&"trip"), "{names:?}");
    assert!(!names.contains(&".secret") && !names.contains(&"escape"), "{names:?}");
    assert_eq!(list["share"], "Shared Photos");
    assert_eq!(list["upload"], false);

    let sub: serde_json::Value = get(format!("{}/api/list?p=trip", f.base)).await.unwrap().json().await.unwrap();
    assert_eq!(sub["crumbs"].as_array().unwrap().len(), 2);
    assert_eq!(sub["items"][0]["name"], "día 1.bin");

    // Whole file, a byte range, HEAD.
    let r = get(format!("{}/f/notes.txt", f.base)).await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["accept-ranges"], "bytes");
    assert_eq!(r.text().await.unwrap(), "0123456789abcdef");
    let r = f.http.get(format!("{}/f/notes.txt", f.base)).header("range", "bytes=4-7").send().await.unwrap();
    assert_eq!(r.status(), 206);
    assert_eq!(r.headers()["content-range"], "bytes 4-7/16");
    assert_eq!(r.text().await.unwrap(), "4567");
    let r = f.http.get(format!("{}/f/notes.txt", f.base)).header("range", "bytes=99-").send().await.unwrap();
    assert_eq!(r.status(), 416);
    let r = f.http.head(format!("{}/f/trip/d%C3%ADa%201.bin", f.base)).send().await.unwrap();
    assert_eq!(r.headers()["content-length"], "300000");
    assert!(r.headers()["content-disposition"].to_str().unwrap().starts_with("attachment"));

    // Active content is served as inert text in a sandbox.
    let r = get(format!("{}/f/page.html", f.base)).await.unwrap();
    assert_eq!(r.headers()["content-type"], "text/plain; charset=utf-8");
    assert!(r.headers()["content-security-policy"].to_str().unwrap().starts_with("sandbox"));
    let r = get(format!("{}/f/page.html?dl=1", f.base)).await.unwrap();
    assert_eq!(r.headers()["content-type"], "application/octet-stream");

    // Nothing outside the folder is reachable, however it's spelled.
    for bad in [
        "/f/..%2Fprivate%2Fpasswords.txt",
        "/f/%2E%2E/private/passwords.txt",
        "/f/trip/..%2F..%2Fprivate%2Fpasswords.txt",
        "/f/escape/passwords.txt",
        "/f/.secret",
        "/f/trip",
        "/api/list?p=..",
        "/api/list?p=escape",
        "/zip/escape",
    ] {
        let r = get(format!("{}{bad}", f.base)).await.unwrap();
        assert!(r.status().is_client_error(), "{bad} -> {}", r.status());
    }
    assert!(f.outside.join("passwords.txt").exists());

    // Wrong token: the page, with 404; the API is unreachable.
    let wrong = format!("http://127.0.0.1:{}/s/aaaaaaaaaaaa", f.shares.port());
    assert_eq!(get(format!("{wrong}/")).await.unwrap().status(), 404);
    assert_eq!(get(format!("{wrong}/f/notes.txt")).await.unwrap().status(), 404);

    let views = f.shares.views();
    assert_eq!(views[0].visitors, 1);
    assert!(views[0].bytes_out >= 16);
    assert!(views[0].activity.iter().any(|a| a.action == "downloaded" && a.name == "notes.txt"));
}

#[tokio::test]
async fn folder_downloads_as_an_exactly_sized_zip() {
    let f = setup(ShareOptions::default()).await;
    let r = f.http.get(format!("{}/zip/", f.base)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-disposition"].to_str().unwrap().contains("Shared Photos.zip"));
    let len: usize = r.headers()["content-length"].to_str().unwrap().parse().unwrap();
    let bytes = r.bytes().await.unwrap();
    assert_eq!(bytes.len(), len);
    let zip = f.root.parent().unwrap().join("out.zip");
    std::fs::write(&zip, &bytes).unwrap();
    if let Ok(o) = std::process::Command::new("unzip").arg("-l").arg(&zip).output() {
        let listing = String::from_utf8_lossy(&o.stdout);
        assert!(o.status.success(), "{listing}");
        assert!(listing.contains("Shared Photos/trip/día 1.bin") || listing.contains("Shared Photos/trip/d"), "{listing}");
        assert!(!listing.contains(".secret") && !listing.contains("passwords"), "{listing}");
        let t = std::process::Command::new("unzip").arg("-t").arg(&zip).output().unwrap();
        assert!(t.status.success(), "{}", String::from_utf8_lossy(&t.stdout));
    }
}

#[tokio::test]
async fn uploads_only_when_allowed_and_never_overwrite() {
    let f = setup(ShareOptions::default()).await;
    let put =
        |name: &str, body: &'static str| f.http.put(format!("{}/up/trip?name={}", f.base, urlencoding::encode(name))).body(body).send();
    assert_eq!(put("new.txt", "x").await.unwrap().status(), 403);

    f.shares.update(&f.id, ShareOptions { allow_upload: true, ..Default::default() }).unwrap();
    let r = put("new.txt", "first").await.unwrap();
    assert_eq!(r.status(), 200);
    let r = put("new.txt", "second").await.unwrap();
    let j: serde_json::Value = r.json().await.unwrap();
    assert_eq!(j["name"], "new 2.txt");
    assert_eq!(std::fs::read_to_string(f.root.join("trip/new.txt")).unwrap(), "first");
    assert_eq!(std::fs::read_to_string(f.root.join("trip/new 2.txt")).unwrap(), "second");
    for bad in ["../evil.txt", ".hidden", "a/b", ""] {
        assert_eq!(put(bad, "x").await.unwrap().status(), 400, "{bad}");
    }
    assert!(!f.root.parent().unwrap().join("evil.txt").exists());
    // No partial files left behind.
    let leftovers: Vec<_> = std::fs::read_dir(f.root.join("trip"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains("5files-part"))
        .collect();
    assert!(leftovers.is_empty());
    assert!(f.shares.views()[0].activity.iter().any(|a| a.action == "uploaded" && a.name == "trip/new 2.txt"));
}

#[tokio::test]
async fn stopping_the_last_share_closes_the_server() {
    let f = setup(ShareOptions::default()).await;
    let port = f.shares.port();
    assert_eq!(f.http.get(format!("{}/f/notes.txt", f.base)).send().await.unwrap().status(), 200);
    f.shares.stop(&f.id).await;
    assert_eq!(f.shares.port(), 0);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    // Either refused, or (if the OS still had it) answered as unknown.
    let r = reqwest::Client::new().get(format!("http://127.0.0.1:{port}/s/{}/f/notes.txt", f.id)).send().await;
    assert!(r.map(|r| r.status() == 404).unwrap_or(true));
}

#[tokio::test]
async fn pin_guards_everything_and_resists_guessing() {
    let f = setup(ShareOptions { pin: Some("482913".into()), ..Default::default() }).await;
    let get = |u: String, cookie: Option<String>| {
        let mut r = f.http.get(u);
        if let Some(c) = cookie {
            r = r.header("cookie", c);
        }
        r.send()
    };
    let pin = |p: &'static str| f.http.post(format!("{}/pin", f.base)).body(format!("{{\"pin\":\"{p}\"}}")).send();

    // The page loads (it asks for the PIN); nothing else does.
    assert_eq!(get(format!("{}/", f.base), None).await.unwrap().status(), 200);
    for u in ["/api/list?p=", "/f/notes.txt", "/zip/", "/t/notes.txt"] {
        let r = get(format!("{}{u}", f.base), None).await.unwrap();
        assert_eq!(r.status(), 401, "{u}");
    }
    let r = get(format!("{}/api/list?p=", f.base), None).await.unwrap();
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["pin"], true);

    // Wrong PINs count down, then the address has to wait.
    for left in [4, 3, 2, 1] {
        let r = pin("000000").await.unwrap();
        assert_eq!(r.status(), 401);
        assert_eq!(r.json::<serde_json::Value>().await.unwrap()["triesLeft"], left);
    }
    let r = pin("000000").await.unwrap();
    assert_eq!(r.status(), 429);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["retryIn"], 30);
    // Even the right PIN is refused while locked.
    assert_eq!(pin("482913").await.unwrap().status(), 429);
    assert!(f.shares.views()[0].activity.iter().any(|a| a.action == "wrongPin"));

    // A fresh share (no lockout): the right PIN sets a session cookie that opens everything.
    let f2 = setup(ShareOptions { pin: Some("4829".into()), ..Default::default() }).await;
    let r = f2.http.post(format!("{}/pin", f2.base)).body(r#"{"pin":"4829"}"#).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let set = r.headers()["set-cookie"].to_str().unwrap().to_string();
    assert!(set.contains("HttpOnly") && set.contains("SameSite=Strict") && set.contains(&format!("Path=/s/{}/", f2.id)), "{set}");
    let cookie = set.split(';').next().unwrap().to_string();
    let ok = |u: &str| {
        let (http, base, c) = (f2.http.clone(), f2.base.clone(), cookie.clone());
        let u = u.to_string();
        async move { http.get(format!("{base}{u}")).header("cookie", c).send().await.unwrap().status() }
    };
    assert_eq!(ok("/api/list?p=").await, 200);
    assert_eq!(ok("/f/notes.txt").await, 200);
    assert!(f2.shares.views()[0].activity.iter().any(|a| a.action == "unlocked"));

    // Changing the PIN signs everyone out.
    f2.shares.update(&f2.id, ShareOptions { pin: Some("1111".into()), ..Default::default() }).unwrap();
    assert_eq!(ok("/api/list?p=").await, 401);
    // Turning it off opens the share again.
    f2.shares.update(&f2.id, ShareOptions::default()).unwrap();
    assert_eq!(f2.http.get(format!("{}/api/list?p=", f2.base)).send().await.unwrap().status(), 200);
    // Bad PINs are rejected when set.
    assert!(f2.shares.update(&f2.id, ShareOptions { pin: Some("12a4".into()), ..Default::default() }).is_err());
    assert!(f2.shares.update(&f2.id, ShareOptions { pin: Some("123".into()), ..Default::default() }).is_err());
}
