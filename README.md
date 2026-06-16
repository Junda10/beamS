# beams

> Beam your localhost to the world — free, friendly, for everyone.

`beams` shares a local HTTP service to a public `https://*.trycloudflare.com`
URL with one command. Free forever, no signup. It auto-downloads `cloudflared`
on first run and prints a QR code so you can open the URL on your phone.

## Install

```bash
# Run instantly, no install (npm package is published as "beams-cli")
npx beams-cli 3000

# Or install globally — the command is `beams`
npm i -g beams-cli      # then:  beams 3000

# Or build from source
cargo install --path .
```

> The npm package is named `beams-cli` because `beams` was already taken on npm,
> but the command you run is always `beams`.

## Usage

```bash
beams 3000                       # random HTTPS URL via Cloudflare (default)
beams http://localhost:8080      # explicit URL

beams 3000 --subdomain myapp     # fixed subdomain -> https://myapp.loca.lt (localtunnel)
beams 22 --tcp                   # raw TCP (SSH, databases, …) -> bore.pub:PORT (bore)
```

Press `Ctrl+C` to stop. Notes:

- The default Cloudflare URL is random and changes each run; quick tunnels take a
  few seconds to become reachable.
- `--subdomain` names are first-come on the shared loca.lt server.
- `--tcp` gives you a random `bore.pub` port for any TCP service.
- Dev servers (Vite, etc.) work out of the box — beams rewrites the `Host` header
  to your local `localhost:PORT`.

## How it works

`beams` dials out to a relay that assigns a public address and forwards traffic
back to your localhost — no inbound ports, no account, no cost. It wraps three
free backends and downloads what it needs on first run:

- **Cloudflare Quick Tunnel** (default) — random `*.trycloudflare.com` HTTPS URL
- **localtunnel** (`--subdomain`) — chosen `*.loca.lt` subdomain
- **bore** (`--tcp`) — raw TCP via `bore.pub`

## Roadmap

- v0.3 — bring-your-own domain; config file for multiple tunnels
- later — background daemon

## License

MIT
