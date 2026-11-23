use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).ok_or("SQLite path required")?;
    let points = pulse_history::replay(&PathBuf::from(path))?;
    assert_eq!(points.len(), 4);
    assert!(points.iter().all(
        |p| p.values[1].is_some_and(|ram| (0. ..=100.).contains(&ram))
            && p.values[2].is_none_or(|gpu| (0. ..=100.).contains(&gpu))
    ));
    println!("SQLite: 4 retained samples, measured RAM, GPU valid when available.");
    assert!(
        points
            .iter()
            .flat_map(|p| p.rates)
            .flatten()
            .all(|bps| bps.is_finite() && bps >= 0.)
    );
    Ok(())
}
