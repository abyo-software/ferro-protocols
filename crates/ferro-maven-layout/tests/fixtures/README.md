# Maven layout conformance fixtures

Real Maven Central artefact metadata used by `tests/conformance.rs`.

## Sources

- `commons-lang3-3.14.0.pom.xml` — excerpt of
  `org.apache.commons:commons-lang3:3.14.0` POM published at
  <https://repo1.maven.org/maven2/org/apache/commons/commons-lang3/3.14.0/commons-lang3-3.14.0.pom>
- `commons-lang3-maven-metadata.xml` — excerpt of the artifact-index
  metadata at
  <https://repo1.maven.org/maven2/org/apache/commons/commons-lang3/maven-metadata.xml>

License compliance: both files are Apache-2.0 licensed (Apache Commons
Lang). Reproduction of POM metadata for interop testing is explicitly
permitted by the Apache License §4.

## Excerpt note

The upstream POM is ~16 KB and includes plugin / profile blocks out of
scope for the GAV / parent / packaging surface this crate parses; we
retain only the elements the parser inspects, plus a representative
dependency edge.

The upstream `maven-metadata.xml` enumerates the full release history
of the artifact (40+ versions from 3.0 onward); we retain the head
through 3.14.0 to keep the file size reasonable while preserving the
`<release>`, `<latest>`, `<versions>`, and `<lastUpdated>` elements in
the real wire shape.
