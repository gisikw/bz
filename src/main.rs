use color_eyre::eyre::Result;

fn main() -> Result<()> {
    color_eyre::install()?;

    println!("bz v{}", env!("CARGO_PKG_VERSION"));
    println!("Scaffold complete. Next: terminal initialization.");

    Ok(())
}
