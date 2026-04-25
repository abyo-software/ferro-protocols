// SPDX-License-Identifier: Apache-2.0
//! Send a small batch of JSON events to a Logstash endpoint.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example send_event -- 127.0.0.1:5044
//! ```
//!
//! Default endpoint is `127.0.0.1:5044` (the standard Logstash Beats
//! input port).

use std::time::Duration;

use ferro_lumberjack::client::ClientBuilder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:5044".to_string());

    eprintln!("ferro-lumberjack: connecting to {host}…");

    let mut client = ClientBuilder::new()
        .add_host(host)
        .compression_level(3)
        .timeout(Duration::from_secs(10))
        .connect()
        .await?;

    let events: Vec<Vec<u8>> = (0..3)
        .map(|i| {
            format!(r#"{{"message":"hello-from-ferro-lumberjack-{i}","level":"info"}}"#)
                .into_bytes()
        })
        .collect();

    let acked = client.send_json(events).await?;
    eprintln!("ferro-lumberjack: receiver acknowledged {acked} events");

    Ok(())
}
