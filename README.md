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
beams 3000                       # forwards http://localhost:3000
beams http://localhost:8080      # explicit URL
```

Press `Ctrl+C` to stop. The public URL is temporary and changes each run.

## How it works

`beams` wraps Cloudflare Quick Tunnel: your machine dials out to Cloudflare,
which assigns a public HTTPS URL and relays traffic back to your localhost. No
inbound ports, no account, no cost.

## Roadmap

- v0.2 — fixed custom subdomain; TCP support (SSH/databases)
- v0.3 — bring-your-own domain; config file for multiple tunnels
- later — background daemon

## License

MIT
