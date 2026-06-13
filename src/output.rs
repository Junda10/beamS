use crate::error::{PharosError, Result};

/// Render a URL as a scannable Unicode QR code string.
pub fn render_qr(url: &str) -> Result<String> {
    use qrcode::render::unicode;
    use qrcode::QrCode;
    let code = QrCode::new(url.as_bytes())
        .map_err(|e| PharosError::Download(format!("生成二维码失败: {e}")))?;
    Ok(code.render::<unicode::Dense1x2>().quiet_zone(true).build())
}

/// Print the success banner with the public URL, forwarding target, and QR code.
pub fn print_banner(public_url: &str, target: &str) -> Result<()> {
    use owo_colors::OwoColorize;
    println!("  {} 你的本地服务已上线！\n", "✓".green());
    println!("  🌐 公网地址:  {}", public_url.bold().cyan());
    println!("  📍 转发到:    {}\n", target);
    println!("{}", render_qr(public_url)?);
    println!("  手机扫码即可访问 · 按 {} 停止", "Ctrl+C".yellow());
    Ok(())
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
