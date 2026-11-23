#![forbid(unsafe_code)]

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Finding {
    Insufficient,
    Stable,
    Growth {
        delta_mib: f64,
        slope_mib_hour: f64,
        duration_ms: u64,
        sustained: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CpuLoad {
    Insufficient,
    BelowThreshold,
    Sustained {
        duration_ms: u64,
        peak_percent: f64,
        count: usize,
    },
}

pub const CPU_LIMIT: f64 = 90.;
pub const CPU_WINDOW_MS: u64 = 15_000;
const MAX_GAP_MS: u64 = 6_000;

/// CPU-01: evaluate the trailing contiguous measurements in reverse time order.
pub fn sustained_cpu(newest_first: impl IntoIterator<Item = (u64, Option<f64>)>) -> CpuLoad {
    let mut iter = newest_first.into_iter();
    let Some((latest, Some(value))) = iter.next() else {
        return CpuLoad::Insufficient;
    };
    if !(0. ..=100.).contains(&value) {
        return CpuLoad::Insufficient;
    }
    let mut newest = latest;
    let mut oldest_high = latest;
    let mut peak = value;
    let mut high_count = usize::from(value >= CPU_LIMIT);
    let mut still_high = high_count == 1;
    for (at, reading) in iter {
        if !newest
            .checked_sub(at)
            .is_some_and(|delta| (1..=MAX_GAP_MS).contains(&delta))
        {
            break;
        }
        let Some(value) = reading.filter(|v| (0. ..=100.).contains(v)) else {
            break;
        };
        newest = at;
        if still_high && value >= CPU_LIMIT {
            oldest_high = at;
            peak = peak.max(value);
            high_count += 1;
        } else {
            still_high = false;
        }
        if !still_high && latest - newest >= CPU_WINDOW_MS {
            break;
        }
    }
    if high_count >= 4 && latest - oldest_high >= CPU_WINDOW_MS {
        CpuLoad::Sustained {
            duration_ms: latest - oldest_high,
            peak_percent: peak,
            count: high_count,
        }
    } else if latest - newest >= CPU_WINDOW_MS {
        CpuLoad::BelowThreshold
    } else {
        CpuLoad::Insufficient
    }
}

/// Rule MEM-01 v1. Input: elapsed milliseconds and measured private bytes of one instance.
pub fn private_growth(points: impl DoubleEndedIterator<Item = (u64, u64)>) -> Finding {
    // Six-second spacing gives at least 30 minutes in the most recent 321 valid points.
    let mut points: Vec<_> = points.rev().take(321).collect();
    points.reverse();
    if points.len() < 20 {
        return Finding::Insufficient;
    }
    let Some(duration_ms) = points.last().unwrap().0.checked_sub(points[0].0) else {
        return Finding::Insufficient;
    };
    if duration_ms < 1_800_000
        || points
            .windows(2)
            .any(|p| p[1].0 <= p[0].0 || p[1].0 - p[0].0 > 120_000)
    {
        return Finding::Insufficient;
    }
    let mib = |b: u64| b as f64 / 1_048_576.;
    let baseline = mib(points[0].1);
    let delta_mib = mib(points.last().unwrap().1) - baseline;
    if delta_mib <= 0. {
        return Finding::Stable;
    }
    // Bound pairwise work while preserving endpoints and even coverage of the window.
    let count = points.len().min(41);
    let reduced = (0..count)
        .map(|i| points[i * (points.len() - 1) / (count - 1)])
        .collect::<Vec<_>>();
    let mut slopes = Vec::with_capacity(count * (count - 1) / 2);
    for (i, &(a, x)) in reduced.iter().enumerate() {
        for &(b, y) in &reduced[i + 1..] {
            slopes.push((mib(y) - mib(x)) * 3_600_000. / (b - a) as f64);
        }
    }
    slopes.sort_unstable_by(f64::total_cmp);
    let slope_mib_hour = slopes[slopes.len() / 2];
    let half = points.len() / 2;
    let late = points.len() * 3 / 4;
    let retained = points[half..]
        .iter()
        .all(|&(_, v)| mib(v) >= baseline + delta_mib * 0.2);
    let progressing = mib(points.last().unwrap().1) - mib(points[late].1) >= delta_mib * 0.1;
    Finding::Growth {
        delta_mib,
        slope_mib_hour,
        duration_ms,
        sustained: delta_mib >= 256. && slope_mib_hour >= 64. && retained && progressing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const M: u64 = 1_048_576;

    #[test]
    fn distinguishes_growth_from_a_bounded_cache_and_release() {
        let scenario =
            |f: fn(u64) -> u64| private_growth((0..=30).map(move |i| (i * 60_000, f(i) * M)));
        assert!(matches!(
            scenario(|i| 512 + i * 10),
            Finding::Growth {
                sustained: true,
                ..
            }
        ));
        assert!(matches!(
            scenario(|i| 512 + i.min(8) * 40),
            Finding::Growth {
                sustained: false,
                ..
            }
        ));
        assert_eq!(scenario(|i| 512 + (i % 30) * 10), Finding::Stable);
        assert_eq!(
            private_growth((0..20).map(|i| (i * 1_000, i * M))),
            Finding::Insufficient
        );
    }

    #[test]
    fn high_cpu_requires_a_contiguous_streak_not_an_isolated_peak() {
        let fixture = [
            (20_000, Some(95.)),
            (15_000, Some(92.)),
            (10_000, Some(96.)),
            (5_000, Some(91.)),
            (0, Some(20.)),
        ];
        assert_eq!(
            sustained_cpu(fixture),
            CpuLoad::Sustained {
                duration_ms: 15_000,
                peak_percent: 96.,
                count: 4
            }
        );
        let mut gap = fixture;
        gap[2].1 = None;
        assert_eq!(sustained_cpu(gap), CpuLoad::Insufficient);
        let mut dip = fixture;
        dip[2].1 = Some(15.);
        assert_eq!(sustained_cpu(dip), CpuLoad::BelowThreshold);
    }
}
