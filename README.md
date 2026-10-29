# deebot-cli-ui

A command-line tool and a desktop and Android app for controlling an Ecovacs Deebot robot through the Ecovacs cloud. Built and tested on a Deebot T50 (class `elrxgb`); other models may differ. Unofficial, and not affiliated with Ecovacs.

## Layout

| Path | What it is |
|---|---|
| `crates/deebot-core` | Library: login, saved session, every robot command, map decoding |
| `src/` (`deebot-cli`) | The command-line tool |
| `crates/deebot-ui` | egui app for macOS and Android, built on the same library |

## CLI

```bash
cargo build --release
./target/release/deebot-cli login          # first login; asks for your password, and an emailed code on a new client
./target/release/deebot-cli status
./target/release/deebot-cli clean start
./target/release/deebot-cli rooms
./target/release/deebot-cli clean-room Kitchen "Living room" --mode vacuum
./target/release/deebot-cli --help         # everything else
```

After the first login only an account token is saved (to `~/.config/deebot-cli/session.json`, readable only by you), never the password. `deebot-cli logout` deletes it. Use `--country` to match the region of your Ecovacs account. Repeated failed logins lock the account for about 30 minutes.

Room cleaning: this robot has no "clean room N" request. It cleans rooms through a `freeClean` command whose value is `1,<room id>` per room, joined by `;`. It also resets to vacuum-and-mop after each stop, so `--mode` sets the mode just before the clean.

## Desktop app

```bash
cargo run --release -p deebot-ui
```

## Android app

Needs the Android SDK and NDK r26, `rustup target add aarch64-linux-android`, and `cargo install cargo-apk`.

```bash
cargo apk build -p deebot-ui --lib
adb install -r target/debug/apk/deebot.apk
```

The package is debug-signed. The launcher icons are drawn by `crates/deebot-ui/tools/make_icons.py`.

## Tests

```bash
cargo test --workspace
```
