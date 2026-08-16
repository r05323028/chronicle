# Spring WebMVC example: record real traffic, replay it

A real [Spring WebMVC](https://docs.spring.io/spring-framework/reference/web/webmvc.html)
application that shows the full Chronicle loop: record live HTTP/1.1 traffic with
eBPF on Linux, publish it as a recording, then replay it against a fresh app
instance and verify responses match.

```text
curl ──> Spring WebMVC (port 18080) ── chronicle record -- java -jar ...
        
        chronicle replay latest -- java -jar ...   (fresh instance)
```

## Layout

| Path | Purpose |
| --- | --- |
| `pom.xml` | Spring Boot 3.3 + `spring-boot-starter-web`, Maven build, jar `target/webmvc.jar` |
| `src/main/java/example/webmvc/` | `WebmvcApplication` + `HelloController` (`GET /hello`, `POST /echo`) |
| `demo.sh` | `build` \| `record` \| `replay` \| `clean` one-liners |
| `fixtures/webmvc-session.json` | deterministic stand-in for capture, for portable hosts (see below) |

## Prerequisites

- **Linux 6.1+** (cgroup v2, BTF) with `CAP_BPF`/`CAP_NET_ADMIN` for live
  capture; run `chronicle doctor` first. macOS/other hosts: portable surface only
  (fixture record + explicit-target replay, below).
- `chronicle` binary, JDK 17+, Maven, `curl`.

## Record then replay (Linux)

```bash
cd examples/spring-webmvc
./demo.sh build        # mvn package -> target/webmvc.jar
./demo.sh record       # spawn app under chronicle, drive traffic, finalize recording
./demo.sh replay       # spawn fresh app instance, replay recorded traffic, verify
```

### What each step does

1. `chronicle record --name webmvc-demo -- java -jar ...` spawns the app in a
   supervised scope and attaches eBPF capture to it. No application
   instrumentation, no proxy, no special mode.
2. `curl` drives one `GET /hello` and one `POST /echo` (the POST is a write, so
   replay later needs `--allow-write`).
3. `SIGTERM` to `chronicle record` stops intake, drains, and finalizes the
   recording (first signal = graceful; a second forces termination).
4. `chronicle inspect latest` shows the published recording: operations, effect
   classification, replayability.
5. `chronicle replay latest -- java -jar ...` spawns a fresh instance, infers
   its loopback listener from owned socket evidence, and replays both
   operations. Command mode auto-grants execution and read effects; writes
   still need `--allow-write`. Verification compares status, body SHA-256, and
   non-ignored headers. Expect `Replay passed.` and exit 0.

## Why this app replays cleanly

- **Deterministic bodies.** Chronicle compares body digests, so the endpoints
  return fixed strings — no timestamps, UUIDs, or random values.
- **Plaintext HTTP/1.1.** That is what live capture and replay support; Spring
  Boot's default Tomcat serves plain HTTP/1.1 (no TLS config needed).
- **Ignored headers.** The HTTP verifier skips `Date`, `Server`,
  `Content-Length`, `Connection`, `Transfer-Encoding`, `Keep-Alive`, and
  `Set-Cookie`; `Content-Type` and other headers must match.

Change the endpoints and the replay expectations change with them — that is the
point: replay is a regression test for the app's real behavior.

## Portable hosts (macOS, CI without eBPF)

Live capture is Linux-only. On any host you can still exercise replay against
this app with the checked-in fixture (it mirrors the exact wire behavior of
`GET /hello` and `POST /echo` against the real app):

```bash
cd examples/spring-webmvc
mvn -q package
java -jar target/webmvc.jar --server.port=18081 &   # second instance, port must differ
chronicle internal record-fixture --input fixtures/webmvc-session.json --root /tmp/webmvc-root
chronicle replay <session-id> --root /tmp/webmvc-root \
  --target http://127.0.0.1:18081 --allow-host 127.0.0.1 \
  --allow-read --allow-write --execute
```

The recorded fixture targets port 18080; replay uses 18081 because explicit
-target mode never replays into the recorded destination (default-deny safety).
Command mode on Linux is exempt: it targets a fresh supervised instance, never
an external destination.
