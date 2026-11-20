# sf-bridge – Stockfish bridge

Lets the patched chess.com app on your phone run Game Review on **the Stockfish of your own computer**
instead of the phone's, over your own Wi-Fi. It is what the *Stockfish on your computer* reviewer in
Quwi's Patches talks to.

Nothing leaves your network: the bridge only waits for the phone and opens no outbound connections.
There are no accounts and no telemetry. It is a single static binary written in Rust.

## What you need

- **Stockfish 17 or newer** — <https://stockfishchess.org/download/> (not needed with the Docker image,
  which includes it).
- Phone and computer on the **same Wi-Fi**, with the router not isolating clients from each other.

## Get it

- **Binaries** for Linux (x86_64, static) and Windows (x86_64): the *Releases* page, or the artifacts of a CI run.
- **Docker**: `ghcr.io/developsessions/sf-bridge:latest`
- **From source**: `cargo build --release`

## Set it up

1. Put `stockfish` (`stockfish.exe` on Windows) next to `sf-bridge`, or point `SF_PATH` at it.
2. Allow the port through your firewall, once:

   - Windows (administrator terminal):
     ```
     netsh advfirewall firewall add rule name="sf-bridge" dir=in action=allow protocol=TCP localport=7892 profile=any remoteip=localsubnet
     ```
     `profile=any` matters: Windows treats a café or office network as public, and a private-only rule
     would be ignored. `remoteip=localsubnet` keeps the port closed to anything outside your network.
   - Linux: `sudo ufw allow 7892/tcp` or `sudo firewall-cmd --add-port=7892/tcp --permanent && sudo firewall-cmd --reload`

3. Start it:

   ```
   ./sf-bridge
   ```

   ```
   sf-bridge: UCI over TCP on port 7892
     on your network: 192.168.1.10:7892  (the address the phone wants)
     cores:           12
     token:           3f9c1a2b…8d41e6   (whole one in /home/you/token.txt)
   Waiting for the phone. Ctrl+C to stop.
   ```

   The first run writes `token.txt` (readable by the owner only, on Unix) in the current folder. That
   file is your password: the phone needs the whole string, and the bridge refuses anyone without it.

4. In the app: **More → Quwi's Patches → Game Review → Stockfish on your computer**, then enter

   - **Computer address** — what the bridge printed after "on your network", e.g. `192.168.1.10:7892`;
   - **Token** — the whole string from `token.txt`;
   - press **Test the connection**: *"The computer answered: its Stockfish is ready."*

   Keep the computer on and the bridge running during a review.

## Options

```
sf-bridge [--port <1-65535>]
```

| Setting | Default | |
|---|---|---|
| `--port`, `-p` | `$TCP_PORT`, else `7892` | listening port |
| `SF_PATH` | `./stockfish[.exe]`, then `PATH` | Stockfish binary |
| `TOKEN_FILE` | `./token.txt` | where the token is kept |

## Docker

```
docker run -d --name sf-bridge --restart unless-stopped -p 7892:7892 -v sf-bridge:/data ghcr.io/developsessions/sf-bridge:latest
docker logs sf-bridge                      # shortened token and address
docker exec sf-bridge cat /data/token.txt  # the whole token
```

Another port: `-e TCP_PORT=9000 -p 9000:9000` (or `--port 9000` after the image name). Inside a container the
bridge prints the container's address; give the phone your computer's LAN address with the published port
(or run with `--network host`). The container runs as an unprivileged user.

## systemd (not as root)

`systemd/sf-bridge.service` runs the bridge as a throwaway unprivileged user (`DynamicUser=yes`), sandboxed,
with the token in `/var/lib/sf-bridge`. Stockfish must not be in a home folder (`ProtectHome`).

```
sudo install -Dm755 sf-bridge /usr/local/bin/sf-bridge
sudo install -Dm755 /path/to/stockfish /usr/local/lib/sf-bridge/stockfish
sudo install -Dm644 systemd/sf-bridge.service /etc/systemd/system/sf-bridge.service
sudo systemctl daemon-reload && sudo systemctl enable --now sf-bridge
journalctl -u sf-bridge -n 20
sudo cat /var/lib/private/sf-bridge/token.txt
```

## Test and benchmark

The release archives and `cargo build` also contain two tools. Run them while the bridge is running, from
the folder with `token.txt` (or set `TOKEN_FILE`); `TEST_HOST` tests another machine on the network.

```
test-tcp     # one position, prints the engine's evaluations;  TEST_NODES, TEST_THREADS
bench-tcp    # 100k / 300k / 1M / 5M nodes a move;               TEST_THREADS
```

`bench-tcp` also times Stockfish's start-up, which is paid once per review, not once per move:

```
Startup (once per review): 0.32 s
threads 32, hash 1024 MB

100k nodes a move: 0.06 s
300k nodes a move: 0.06 s
1M   nodes a move: 0.08 s
5M   nodes a move: 0.20 s
```

## The protocol

Port **7892**. It is UCI with one handshake in front, one command per line:

```
server: sf-bridge 1 cores 12
client: auth <token>
server: auth ok            (or: auth fail, and the connection is closed)
... plain UCI, both ways
```

The greeting carries the core count so that *Auto* threads on the phone means this machine's cores. Each
connection starts its own Stockfish and keeps it for the whole review, so every move is searched by the same
warm engine; it is told to quit when the phone hangs up.

Safeguards: the token is compared in constant time, a connection that does not authenticate within 10 s is
dropped, lines over 1 MiB drop the connection, and at most 16 connections are served at once. It is the only
port the bridge opens.

## If the phone does not see the computer

The bridge prints a line whenever a connection arrives, so the console tells you which half is broken:

1. Both on the same Wi-Fi; turn mobile data off on the phone for the test.
2. *Test the connection* says it could not connect and **nothing** appeared in the bridge's console: the phone
   never reached the computer. It is the firewall rule or the router isolating its clients, not the app.
3. "The bridge refused the token": `token.txt` and the token on the phone differ (one character is enough).
   Copy it again, or delete `token.txt`, restart the bridge and enter the new one.
4. Timeouts in the middle of a review usually mean the computer went to sleep.
5. The computer's address changes when it reconnects unless the router reserves one for it (DHCP reservation).

## CI

`.github/workflows/ci.yml` runs rustfmt, clippy and the build on every push and pull request, builds the Linux
and Windows binaries, and pushes the Docker image to GHCR from `main` (`latest`) and from version tags
(`1.2.3`, `1.2`). A tag `vX.Y.Z` also creates a GitHub release with both binaries attached.

## License

[GNU General Public License v3.0](LICENSE) (`GPL-3.0-only`).
