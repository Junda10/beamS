use crate::error::{BeamsError, Result};

/// Render a URL as a scannable Unicode QR code string.
pub fn render_qr(url: &str) -> Result<String> {
    use qrcode::render::unicode;
    use qrcode::QrCode;
    let code = QrCode::new(url.as_bytes())
        .map_err(|e| BeamsError::Download(format!("failed to render QR code: {e}")))?;
    Ok(code.render::<unicode::Dense1x2>().quiet_zone(true).build())
}

/// Print the success banner with the public URL, forwarding target, and QR code.
/// `ready` is false when the tunnel never became reachable from this machine —
/// say so instead of claiming it is live.
pub fn print_banner(public_url: &str, target: &str, copied: bool, ready: bool) -> Result<()> {
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
    println!("{}", render_qr(public_url)?);
    println!(
        "  Scan the QR code from your phone · press {} to stop",
        "Ctrl+C".yellow()
    );
    Ok(())
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
}
