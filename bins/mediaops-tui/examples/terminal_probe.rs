use anyhow::Result;
use mediaops_tui::terminal::{TerminalGuard, install_panic_hook};

fn main() -> Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "normal".into());
    anyhow::ensure!(
        matches!(mode.as_str(), "normal" | "error" | "panic"),
        "usage: terminal_probe [normal|error|panic]"
    );
    install_panic_hook();
    let (guard, mut terminal) = TerminalGuard::enter()?;
    terminal.draw(|frame| {
        frame.render_widget(ratatui::widgets::Paragraph::new("probe"), frame.area());
    })?;
    match mode.as_str() {
        "panic" => panic!("intentional terminal restoration probe"),
        "error" => anyhow::bail!("intentional terminal error probe"),
        _ => {}
    }
    drop(guard);
    println!("restored");
    Ok(())
}
