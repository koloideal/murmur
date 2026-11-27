## Android builds (mobile/)

The system JDK 27 breaks the Kotlin compiler (`JavaVersion.parse` chokes on
the 4-component version `27.0.1.1` — the error message is just the bare
version string). A working Temurin 21 lives in `~/.local/jdks/jdk-21.0.13.1+2`
and `~/.gradle/gradle.properties` (user-global, not in the repo) already points
gradle at it via `org.gradle.java.home`. If that directory is ever missing: download
`https://api.adoptium.net/v3/binary/latest/21/ga/linux/x64/jdk/hotspot/normal/eclipse`
and unpack it there.

Verify with: `cd mobile && ./gradlew testDebugUnitTest compileDebugKotlin`

## Verify Rust side

`cd laptop && cargo test -p murmurd` — 15 unit+integration tests (election,
dedupe, anti-echo, in-memory duplex, key rotation e2e). Golden wire vectors:
`cd shared/proto && cargo test` — byte-exact against frozen vectors.json.

## Daemon control

`systemctl --user restart murmurd` after rebuilding `laptop/target/debug/murmurd`.
murmurctl exit codes: 0 ok · 2 daemon down · 3 daemon-reported failure ·
4 target unreachable — scripts branch on these, not on prose.
