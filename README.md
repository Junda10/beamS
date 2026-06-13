# pharos

> Like the lighthouse of Alexandria — make your localhost visible to the world.

`pharos` shares a local HTTP service to a public `https://*.trycloudflare.com`
URL with one command. Free forever, no signup. It auto-downloads `cloudflared`
on first run and prints a QR code so you can open the URL on your phone.

## Install

```bash
cargo install --path .
```

## Usage

```bash
pharos 3000                       # forwards http://localhost:3000
pharos http://localhost:8080      # explicit URL
```

Press `Ctrl+C` to stop. The public URL is temporary and changes each run.

## How it works

`pharos` wraps Cloudflare Quick Tunnel: your machine dials out to Cloudflare,
which assigns a public HTTPS URL and relays traffic back to your localhost. No
inbound ports, no account, no cost.

## Roadmap

- v0.2 — fixed custom subdomain; TCP support (SSH/databases)
- v0.3 — bring-your-own domain; config file for multiple tunnels
- later — background daemon

## License

MIT
