// SPDX-License-Identifier: Apache-2.0
//! Minimal Lumberjack v2 receiver that prints every received event to
//! stdout. Suitable as a smoke-test endpoint when developing a sender
//! against this crate.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example echo_server -- 127.0.0.1:5044
//! ```
//!
//! Default bind is `127.0.0.1:5044` (the standard Beats input port).
//! In a separate terminal, point a Beats agent — or
//! `cargo run --example send_event` — at the same address.

use ferro_lumberjack::server::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:5044".to_string());

    let listener = Server::builder().bind(&bind).await?;
    eprintln!(
        "ferro-lumberjack echo_server: listening on {}",
        listener.local_addr()?
    );

    loop {
        let mut conn = listener.accept().await?;
        let peer = conn.peer();
        tokio::spawn(async move {
            eprintln!("connected: {peer}");
            loop {
                match conn.read_window().await {
                    Ok(Some(window)) => {
                        for event in &window.events {
                            // Best-effort UTF-8 decode for printing.
                            let s = String::from_utf8_lossy(&event.payload);
                            println!("  seq={} payload={s}", event.seq);
                        }
                        if let Err(e) = conn.send_ack(window.last_seq).await {
                            eprintln!("ack failed: {e}");
                            break;
                        }
                    }
                    Ok(None) => {
                        eprintln!("disconnected: {peer}");
                        break;
                    }
                    Err(e) => {
                        eprintln!("read error from {peer}: {e}");
                        break;
                    }
                }
            }
        });
    }
}
