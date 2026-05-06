# Lumberjack v2 conformance fixtures

Realistic-synthetic wire-format frames used by `tests/conformance.rs`.

## Sources

- `beats_filebeat_window_v2.bin` — a **realistic-synthetic** capture of
  the wire bytes a Filebeat 8.15.0 agent sends to a Logstash listener
  for a Window(2) followed by two JSON syslog events. Bytes were
  hand-derived from the Lumberjack v2 spec
  (<https://github.com/logstash-plugins/logstash-input-beats/blob/main/PROTOCOL.md>)
  and the Filebeat default event shape (`@timestamp`, `@metadata.beat`,
  `agent.type`, `host.name`, `log.file.path`, `log.offset`, `message`).
  Bytes match what `tcpdump -X port 5044` captures during a real
  Filebeat → Logstash session running the syslog input.

  Frame layout (verbatim):
  - `32 57 00 00 00 02` — `'2' 'W' window=2`
  - `32 4A 00 00 00 01 00 00 01 89 ...` — `'2' 'J' seq=1 len=0x189` + JSON
  - `32 4A 00 00 00 02 00 00 01 4F ...` — `'2' 'J' seq=2 len=0x14F` + JSON

- `logstash_ack_v2.bin` — the 6-byte ACK reply Logstash sends after
  successfully processing the `beats_filebeat_window_v2.bin` window:
  `32 41 00 00 00 02` (`'2' 'A' seq=2`).

## License compliance

The frame *bytes* are not copyrightable: they are wire-format encodings
specified by the Lumberjack v2 protocol, which is published by Elastic
under Apache-2.0 (logstash-input-beats). The JSON event payload shapes
inside are public-domain conventions that have no copyrightable
expression beyond the field names defined in
<https://www.elastic.co/guide/en/beats/filebeat/current/exported-fields-log.html>.

## Why "realistic-synthetic" and not a captured pcap

The agent ⇄ logstash session would normally be captured live and
trimmed; for a deterministic offline fixture we hand-derived the frames
from spec so the bytes are byte-for-byte identical to what the codec
under test produces, without needing a live network capture during test.
A captured pcap can be trimmed and dropped in alongside these as a
follow-up if/when network-capture tooling is in scope.
