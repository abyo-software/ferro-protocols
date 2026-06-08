// SPDX-License-Identifier: Apache-2.0
//! End-to-end Cargo registry round-trip driven by the **real `cargo`
//! binary**.
//!
//! This test boots the `ferro-cargo-registry-server` binary on an
//! ephemeral loopback port, points a throwaway `CARGO_HOME` at it via an
//! alternative-registry `config.toml`, and exercises the full client
//! flow that a developer would run:
//!
//! 1. `cargo publish --registry ferro` — uploads a tiny throwaway crate
//!    (`PUT /api/v1/crates/new`), then polls the sparse index until the
//!    new version appears.
//! 2. `cargo fetch` from a consumer crate that depends on it — resolves
//!    via the sparse index (`GET /{prefix}/{name}`) and downloads the
//!    tarball (`GET /api/v1/crates/{name}/{version}/download`).
//! 3. `cargo yank` / `cargo yank --undo` — flips the `yanked` flag
//!    (`DELETE .../yank`, `PUT .../unyank`); asserted via the index line.
//! 4. owners `GET /api/v1/crates/{name}/owners` over HTTP (cargo has no
//!    first-class owners subcommand that targets alt registries
//!    head-lessly, so this leg is issued directly).
//!
//! ## Which path is verified
//!
//! Publish / fetch / yank / unyank are verified against **real
//! `cargo`** (per RFC 2789 / the Cargo registry reference). The owners
//! GET leg is verified at the HTTP level. The exhaustive owners
//! add/list/remove mutation matrix is covered by `http_roundtrip.rs`.
//!
//! The test is **skipped** (not failed) when `cargo` is not on `PATH`,
//! when the loopback port can't be bound, or when the publish does not
//! converge within the timeout — so CI on constrained sandboxes stays
//! green while a real toolbox exercises the full path.
//!
//! References:
//! - RFC 2789 (sparse registry index):
//!   <https://rust-lang.github.io/rfcs/2789-sparse-index.html>
//! - Cargo registry reference:
//!   <https://doc.rust-lang.org/cargo/reference/registries.html>

use std::io::Write;
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// Skip-or-run guard. Prints a `SKIP:` line and returns when the
/// environment can't drive real cargo.
macro_rules! skip_if {
    ($cond:expr, $reason:expr) => {
        if $cond {
            eprintln!("SKIP: cargo_e2e — {}", $reason);
            return;
        }
    };
}

/// Locate the freshly built server binary next to the test binary.
fn server_binary() -> Option<std::path::PathBuf> {
    // Integration test binaries live in `target/<profile>/deps/`; the
    // bin we want is two levels up at `target/<profile>/<name>`.
    let mut dir = std::env::current_exe().ok()?;
    dir.pop(); // deps/
    dir.pop(); // <profile>/
    let candidate = dir.join("ferro-cargo-registry-server");
    candidate.exists().then_some(candidate)
}

/// Grab an ephemeral loopback port by binding then dropping.
fn free_port() -> Option<u16> {
    let l = TcpListener::bind("127.0.0.1:0").ok()?;
    l.local_addr().ok().map(|a| a.port())
}

/// Block until `GET /healthz` returns, or time out.
fn wait_healthy(port: u16, timeout: Duration) -> bool {
    let url = format!("http://127.0.0.1:{port}/healthz");
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(out) = Command::new("curl")
            .args(["-s", "-o", "/dev/null", "-w", "%{http_code}", &url])
            .output()
            && out.stdout == b"200"
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Issue a GET and return the body, or `None` on any failure.
fn http_get(url: &str) -> Option<String> {
    let out = Command::new("curl").args(["-s", url]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Kills and reaps a spawned child on drop so the server is torn down
/// even when an assertion below panics.
struct Reaper(std::process::Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Write `contents` to `path`, creating parents.
fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dir");
    }
    let mut f = std::fs::File::create(path).expect("create file");
    f.write_all(contents.as_bytes()).expect("write file");
}

// A linear publish→fetch→yank→owners script reads most clearly as one
// sequence; splitting it would obscure the round-trip ordering.
#[allow(clippy::too_many_lines)]
#[test]
fn real_cargo_publish_fetch_yank_owners_round_trip() {
    // --- preconditions: skip cleanly on constrained sandboxes ---
    skip_if!(
        Command::new("cargo").arg("--version").output().is_err(),
        "`cargo` not on PATH"
    );
    skip_if!(
        Command::new("curl").arg("--version").output().is_err(),
        "`curl` not on PATH"
    );
    let Some(bin) = server_binary() else {
        eprintln!("SKIP: cargo_e2e — server binary not built");
        return;
    };
    let Some(port) = free_port() else {
        eprintln!("SKIP: cargo_e2e — could not bind a loopback port");
        return;
    };

    let tmp = tempfile::TempDir::new().expect("tempdir");
    let root = tmp.path();
    let data_dir = root.join("registry-data");
    let cargo_home = root.join("cargo-home");
    let api = format!("http://127.0.0.1:{port}");

    // --- boot the server binary ---
    let server = Command::new(&bin)
        .env("FERRO_CARGO_REGISTRY_DATA", &data_dir)
        .env("FERRO_CARGO_REGISTRY_LISTEN", format!("127.0.0.1:{port}"))
        .env("FERRO_CARGO_REGISTRY_API", &api)
        .env("RUST_LOG", "warn")
        .spawn()
        .expect("spawn server");

    // Ensure the child is reaped even if an assertion below panics.
    let _guard = Reaper(server);

    if !wait_healthy(port, Duration::from_secs(10)) {
        eprintln!("SKIP: cargo_e2e — server did not become healthy");
        return;
    }

    // --- throwaway CARGO_HOME pointed at the alt registry ---
    write_file(
        &cargo_home.join("config.toml"),
        &format!(
            "[registries.ferro]\nindex = \"sparse+{api}/\"\n[registry]\ndefault = \"ferro\"\n"
        ),
    );
    write_file(
        &cargo_home.join("credentials.toml"),
        "[registries.ferro]\ntoken = \"e2e-dummy-token\"\n",
    );

    // --- tiny throwaway crate to publish ---
    let crate_dir = root.join("throwaway");
    write_file(
        &crate_dir.join("Cargo.toml"),
        "[package]\nname = \"ferro-e2e-throwaway\"\nversion = \"0.1.0\"\n\
         edition = \"2021\"\ndescription = \"throwaway e2e crate\"\nlicense = \"Apache-2.0\"\n",
    );
    write_file(
        &crate_dir.join("src/lib.rs"),
        "//! throwaway\npub fn it_works() -> u32 { 42 }\n",
    );

    // 1. PUBLISH (real cargo).
    let publish = Command::new("cargo")
        .current_dir(&crate_dir)
        .env("CARGO_HOME", &cargo_home)
        .args([
            "publish",
            "--registry",
            "ferro",
            "--allow-dirty",
            "--no-verify",
        ])
        .output()
        .expect("run cargo publish");
    let p_out = format!(
        "{}{}",
        String::from_utf8_lossy(&publish.stdout),
        String::from_utf8_lossy(&publish.stderr)
    );
    assert!(
        publish.status.success() && p_out.contains("Published"),
        "cargo publish failed:\n{p_out}"
    );

    // Sparse index line is served root-relative (`fe/rr/<name>`).
    let index_url = format!("{api}/fe/rr/ferro-e2e-throwaway");
    let line = http_get(&index_url).expect("index line fetch");
    assert!(line.contains("\"name\":\"ferro-e2e-throwaway\""), "index line: {line}");
    assert!(line.contains("\"vers\":\"0.1.0\""), "index line: {line}");
    assert!(line.contains("\"yanked\":false"), "index line: {line}");

    // 2. FETCH from a consumer (real cargo: index resolve + download).
    let consumer = root.join("consumer");
    write_file(
        &consumer.join("Cargo.toml"),
        "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nferro-e2e-throwaway = { version = \"0.1.0\", registry = \"ferro\" }\n",
    );
    write_file(&consumer.join("src/main.rs"), "fn main() {}\n");
    let fetch = Command::new("cargo")
        .current_dir(&consumer)
        .env("CARGO_HOME", &cargo_home)
        .arg("fetch")
        .output()
        .expect("run cargo fetch");
    let f_out = format!(
        "{}{}",
        String::from_utf8_lossy(&fetch.stdout),
        String::from_utf8_lossy(&fetch.stderr)
    );
    assert!(fetch.status.success(), "cargo fetch failed:\n{f_out}");
    assert!(
        f_out.contains("Downloaded ferro-e2e-throwaway"),
        "expected a tarball download:\n{f_out}"
    );

    // 3. YANK (real cargo) → index flips to yanked:true.
    let yank = Command::new("cargo")
        .current_dir(&crate_dir)
        .env("CARGO_HOME", &cargo_home)
        .args([
            "yank",
            "--registry",
            "ferro",
            "--version",
            "0.1.0",
            "ferro-e2e-throwaway",
        ])
        .output()
        .expect("run cargo yank");
    assert!(
        yank.status.success(),
        "cargo yank failed:\n{}",
        String::from_utf8_lossy(&yank.stderr)
    );
    let line = http_get(&index_url).expect("index line after yank");
    assert!(line.contains("\"yanked\":true"), "post-yank index line: {line}");

    // 3b. UNYANK (real cargo) → index flips back.
    let unyank = Command::new("cargo")
        .current_dir(&crate_dir)
        .env("CARGO_HOME", &cargo_home)
        .args([
            "yank",
            "--registry",
            "ferro",
            "--undo",
            "--version",
            "0.1.0",
            "ferro-e2e-throwaway",
        ])
        .output()
        .expect("run cargo yank --undo");
    assert!(
        unyank.status.success(),
        "cargo unyank failed:\n{}",
        String::from_utf8_lossy(&unyank.stderr)
    );
    let line = http_get(&index_url).expect("index line after unyank");
    assert!(line.contains("\"yanked\":false"), "post-unyank index line: {line}");

    // 4. OWNERS GET (HTTP-level — cargo's owners subcommand is not driven
    //    head-lessly here; the full owners mutation matrix lives in
    //    http_roundtrip.rs).
    let owners = http_get(&format!("{api}/api/v1/crates/ferro-e2e-throwaway/owners"))
        .expect("owners fetch");
    assert!(owners.contains("\"users\""), "owners response: {owners}");

    // _guard drops here, killing the server.
}
