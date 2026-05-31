slint::include_modules!();

mod keymap;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let ui = ViewerWindow::new()?;
    ui.run()?;
    Ok(())
}
