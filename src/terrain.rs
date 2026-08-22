//! Stub — implementation pending.

use anyhow::Result;
use crate::session::Session;

pub fn run(session: &mut Session) -> Result<()> {
    let _ = session;
    println!("not implemented yet");
    Ok(())
}

#[cfg(test)]
mod probe {
    use cubiomes::enums::{Dimension, MCVersion};
    use cubiomes::generator::{Generator, GeneratorFlags};
    use cubiomes::noise::{BiomeNoise, SurfaceNoiseRelease};

    #[test]
    fn probe_heights() {
        for (label, mc) in [
            ("1.21.1", MCVersion::MC_1_21_1),
            ("1.18.2", MCVersion::MC_1_18_2),
            ("1.17.1", MCVersion::MC_1_17_1),
            ("1.16.5", MCVersion::MC_1_16_5),
        ] {
            let g = Generator::new(mc, 1234, Dimension::DIM_OVERWORLD, GeneratorFlags::empty());
            let n: BiomeNoise = SurfaceNoiseRelease::new(Dimension::DIM_OVERWORLD, 1234).into();
            // 64 nodes at 1:4 == 256 blocks, starting 512 blocks out.
            let v = g.approx_surface_noise(128, 128, 64, 64, &n).expect("some");
            let mut lo = f32::MAX;
            let mut hi = f32::MIN;
            let mut sum = 0.0f64;
            for h in &v {
                lo = lo.min(*h);
                hi = hi.max(*h);
                sum += *h as f64;
            }
            println!("{label}: min {lo:.1} max {hi:.1} mean {:.1}", sum / v.len() as f64);
            // Spot check against the biome at the same spot.
            for i in [0usize, 17, 40] {
                let (nx, nz) = ((128 + i as i32) * 4, (128 + i as i32) * 4);
                let b = g.get_biome_at(nx, 63, nz).unwrap();
                println!("   ({nx},{nz}) h={:.1} biome={:?}", v[i * 64 + i], b);
            }
        }
    }
}
