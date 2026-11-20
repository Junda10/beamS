use crate::error::{BeamsError, Result};

/// Render a URL as a scannable Unicode QR code string.
pub fn render_qr(url: &str) -> Result<String> {
    use qrcode::render::unicode;
    use qrcode::QrCode;
    let code = QrCode::new(url.as_bytes())
        .map_err(|e| BeamsError::Download(format!("failed to render QR code: {e}")))?;
    Ok(code.render::<unicode::Dense1x2>().quiet_zone(true).build())
}

/// Pin the QR code to black on white. Its block glyphs take the terminal's own
/// colours, so on a dark theme the code comes out inverted (light modules on a
/// dark margin), which some phone scanners can't read. Black on bright white,
/// margin included, scans on any theme.
pub fn paint_qr(qr: &str) -> String {
    qr.lines()
        .map(|line| format!("\x1b[30;107m{line}\x1b[0m"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Colour the QR code only for a terminal, and never when NO_COLOR is set.
fn paint_qr_for_stdout() -> bool {
    use std::io::IsTerminal;
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

/// Print the success banner with the public URL, forwarding target, and QR code.
/// `ready` is false when the tunnel never became reachable from this machine —
/// say so instead of claiming it is live.
pub fn print_banner(public_url: &str, target: &str, copied: bool, ready: bool) {
    use owo_colors::OwoColorize;
    if ready {
        println!("  {} Your local service is live!\n", "✓".green());
    } else {
        println!(
            "  {} Tunnel is up, but not reachable from this machine yet\n",
            "!".yellow()
        );
    }
    println!(
        "  🌐 Public URL:  {}{}",
        public_url.bold().cyan(),
        if copied { "  (copied)" } else { "" }
    );
    println!("  📍 Forwarding:  {}\n", target);
    // The QR code is a convenience: if the URL won't fit in one, share without it.
    match render_qr(public_url) {
        Ok(qr) if paint_qr_for_stdout() => println!("{}", paint_qr(&qr)),
        Ok(qr) => println!("{qr}"),
        Err(_) => {
            println!("  Press {} to stop", "Ctrl+C".yellow());
            return;
        }
    }
    println!(
        "  Scan the QR code from your phone · press {} to stop",
        "Ctrl+C".yellow()
    );
}

/// Print the banner for a raw TCP tunnel (no QR code; show the host:port).
pub fn print_tcp_banner(public_addr: &str, local_port: u16, copied: bool, ready: bool) {
    use owo_colors::OwoColorize;
    if ready {
        println!("  {} Your TCP service is live!\n", "✓".green());
    } else {
        println!(
            "  {} Tunnel is up, but not reachable from this machine yet\n",
            "!".yellow()
        );
    }
    println!(
        "  🔌 Public address:  {}{}",
        public_addr.bold().cyan(),
        if copied { "  (copied)" } else { "" }
    );
    println!("  📍 Forwarding:      localhost:{local_port}\n");
    println!(
        "  Connect e.g.  nc {}  ·  press {} to stop",
        public_addr.replace(':', " "),
        "Ctrl+C".yellow()
    );
}

/// One line per request that reached the local server through the tunnel.
pub fn print_request(request: &str) {
    use owo_colors::OwoColorize;
    println!("  {} {request}", "→".dimmed());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_qr_returns_non_empty_block() {
        let qr = render_qr("https://happy-cat-42.trycloudflare.com").unwrap();
        assert!(!qr.trim().is_empty());
        // Unicode QR uses block glyphs; the output should contain newlines (multi-row).
        assert!(qr.contains('\n'));
    }

    #[test]
    fn paint_qr_pins_every_row_to_black_on_white() {
        let qr = render_qr("https://happy-cat-42.trycloudflare.com").unwrap();
        let painted = paint_qr(&qr);
        assert_eq!(painted.lines().count(), qr.lines().count());
        for (row, raw) in painted.lines().zip(qr.lines()) {
            assert_eq!(row, format!("\x1b[30;107m{raw}\x1b[0m"));
        }
        // Rows must be equally wide, or the white background has ragged edges
        // and the quiet zone breaks.
        let widths: Vec<usize> = qr.lines().map(|l| l.chars().count()).collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]), "{widths:?}");
    }
}
