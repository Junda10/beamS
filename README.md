# beams

> Share your localhost with the world — free, no signup.

Start your dev server, run `beams`, and get a public HTTPS link you can send to
anyone or open on your phone.

## Install

You need [Node.js](https://nodejs.org) (for `npm`). Then:

```bash
npm install -g beams-cli
```

The package is called `beams-cli`, but the command is `beams`.

Don't want to install? Run it once with `npx beams-cli`.

**Update** to the latest version:

```bash
npm install -g beams-cli@latest
beams --version
```

## Use it

1. Start your project as usual, e.g. `npm run dev`.
2. In another terminal, in any folder, run:

   ```bash
   beams
   ```

3. Open the link it prints. It's already copied to your clipboard, and there's a
   QR code for your phone.

Press `Ctrl+C` to stop sharing.

`beams` finds your dev server by itself (ports 3000, 5173, 8080, 8000, 4200,
5000, 1313, 4321). If several are running it asks which one to share. You can
also name the port:

```bash
beams 3000
```

While it runs you'll see each visit (`→ GET /about`), and a warning if your dev
server stops.

## More options

```bash
beams 3000 --open              # also open the link in your browser
beams 3000 --subdomain myapp   # pick the name: https://myapp.loca.lt
beams 22 --tcp                 # share a raw TCP port (SSH, databases)
beams 3000 --protocol http2    # use this if the link is slow or keeps dropping
```

## Troubleshooting

- **"nothing is listening on …"** — your dev server isn't running on that port.
  Start it first, or pass the right port.
- **"not reachable from this machine yet"** — the link is usually fine within a
  minute. If your browser still can't open it, clear the DNS cache:
  `ipconfig /flushdns` (Windows), `sudo killall -HUP mDNSResponder` (macOS).
- **Slow or keeps disconnecting** — try `--protocol http2`. Some Wi-Fi and
  office networks block the default connection type.
- The link changes every time you run `beams`. If the connection drops, beams
  reconnects by itself and prints the new link.

## How it works

`beams` connects out to a free relay that gives you a public address and sends
visitors back to your computer. You don't open any ports and there's no
account. By default it uses a Cloudflare Quick Tunnel (`*.trycloudflare.com`);
`--subdomain` uses localtunnel and `--tcp` uses bore. The tools it needs are
downloaded automatically the first time.

## Build from source

```bash
cargo install --path .
```

## License

MIT
