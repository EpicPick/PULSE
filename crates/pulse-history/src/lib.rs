#![forbid(unsafe_code)]
mod session;
pub use session::{SessionCommand, SessionReply, SessionWriter};

use pulse_domain::{CONTRIBUTOR_LIMIT, Contributor, ProcessKey};
use rusqlite::{Connection, OpenFlags, params};
use std::{
    collections::VecDeque,
    fs::OpenOptions,
    io::{self, Write},
    path::Path,
    sync::Arc,
};

pub const MAX: usize = 86_400;
pub const LIVE_CAPACITY: usize = 900;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub utc_ms: u64,
    pub elapsed_ms: u64,
    pub values: [Option<f64>; 3],
    pub rates: [Option<f64>; 2],
}

#[derive(Debug, Clone)]
pub struct ContributorPoint {
    pub elapsed_ms: u64,
    pub entries: Arc<[Contributor]>,
}

#[derive(Debug, Clone, Default)]
pub struct Replay {
    pub points: VecDeque<Point>,
    pub contributors: VecDeque<ContributorPoint>,
}

pub fn ring_push(history: &mut VecDeque<Point>, point: Point) {
    if history.len() >= LIVE_CAPACITY {
        history.pop_front();
    }
    history.push_back(point);
}

pub struct Recording {
    conn: Connection,
    retain: usize,
    count: i64,
}

impl Recording {
    pub fn create(path: &Path, retain: usize) -> Result<Self, Box<dyn std::error::Error>> {
        if !(1..=MAX).contains(&retain) {
            return Err(format!("Rétention hors de l’intervalle 1..={MAX}").into());
        }
        // Never reuse or truncate an existing file supplied on the command line.
        OpenOptions::new().write(true).create_new(true).open(path)?;
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
            PRAGMA wal_autocheckpoint=64; PRAGMA journal_size_limit=524288;
             PRAGMA max_page_count=8192; PRAGMA foreign_keys=ON;
             BEGIN; PRAGMA application_id=1347767379; PRAGMA user_version=3;
             CREATE TABLE system_point (
                 seq INTEGER PRIMARY KEY, utc_ms INTEGER NOT NULL, elapsed_ms INTEGER NOT NULL,
                  cpu REAL, ram REAL, gpu REAL, disk_bps REAL, network_bps REAL,
                  processes_recorded INTEGER NOT NULL CHECK(processes_recorded IN (0,1))
             ) STRICT;
             CREATE TABLE contributor (
                seq INTEGER NOT NULL REFERENCES system_point(seq) ON DELETE CASCADE,
                rank INTEGER NOT NULL CHECK(rank >= 0 AND rank < 12),
                pid INTEGER NOT NULL, creation_ticks INTEGER NOT NULL,
                name TEXT NOT NULL CHECK(length(name) <= 260),
                cpu REAL, private_bytes INTEGER, PRIMARY KEY(seq,rank)
             ) STRICT, WITHOUT ROWID; COMMIT;",
        )?;
        Ok(Self {
            conn,
            retain,
            count: 0,
        })
    }

    pub fn append(&mut self, p: Point) -> rusqlite::Result<()> {
        self.append_with_contributors(p, None)
    }

    pub fn append_with_contributors(
        &mut self,
        p: Point,
        contributors: Option<&[Contributor]>,
    ) -> rusqlite::Result<()> {
        let as_sql =
            |n| i64::try_from(n).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)));
        let next = self.count + 1;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO system_point VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                next,
                as_sql(p.utc_ms)?,
                as_sql(p.elapsed_ms)?,
                p.values[0],
                p.values[1],
                p.values[2],
                p.rates[0],
                p.rates[1],
                contributors.is_some()
            ],
        )?;
        if let Some(contributors) = contributors.filter(|v| !v.is_empty()) {
            let mut insert =
                tx.prepare_cached("INSERT INTO contributor VALUES (?1,?2,?3,?4,?5,?6,?7)")?;
            for (rank, entry) in contributors.iter().take(CONTRIBUTOR_LIMIT).enumerate() {
                insert.execute(params![
                    next,
                    rank as i64,
                    entry.key.pid,
                    as_sql(entry.key.created_at_ticks)?,
                    entry.name,
                    entry.cpu_percent,
                    entry.private_bytes.map(as_sql).transpose()?
                ])?;
            }
        }
        if next > self.retain as i64 {
            tx.execute(
                "DELETE FROM system_point WHERE seq <= ?1",
                [next - self.retain as i64],
            )?;
        }
        tx.commit()?;
        self.count = next;
        Ok(())
    }
}

fn open_session(path: &Path) -> Result<(Connection, i64), Box<dyn std::error::Error>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let app: i64 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if app != 1347767379 || !matches!(version, 1..=3) {
        return Err("Format de session PULSE non reconnu".into());
    }
    Ok((conn, version))
}

fn query(version: i64, order: &str) -> String {
    let rates = if version == 1 {
        "NULL, NULL"
    } else {
        "disk_bps, network_bps"
    };
    format!(
        "SELECT utc_ms, elapsed_ms, cpu, ram, gpu, {rates} FROM system_point ORDER BY seq {order} LIMIT ?1"
    )
}

fn read_point(r: &rusqlite::Row<'_>) -> rusqlite::Result<Point> {
    Ok(Point {
        utc_ms: u64::try_from(r.get::<_, i64>(0)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
        elapsed_ms: u64::try_from(r.get::<_, i64>(1)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        values: [r.get(2)?, r.get(3)?, r.get(4)?],
        rates: [r.get(5)?, r.get(6)?],
    })
}

pub fn replay(path: &Path) -> Result<VecDeque<Point>, Box<dyn std::error::Error>> {
    let (conn, version) = open_session(path)?;
    let mut stmt = conn.prepare(&query(version, "DESC"))?;
    let mut points = stmt
        .query_map([LIVE_CAPACITY as i64], read_point)?
        .collect::<Result<Vec<_>, _>>()?;
    points.reverse();
    Ok(points.into())
}

pub fn replay_with_contributors(path: &Path) -> Result<Replay, Box<dyn std::error::Error>> {
    let (mut conn, version) = open_session(path)?;
    let tx = conn.transaction()?;
    let (points, contributors) = {
        let mut stmt = tx.prepare(&query(version, "DESC"))?;
        let mut points = stmt
            .query_map([LIVE_CAPACITY as i64], read_point)?
            .collect::<Result<Vec<_>, _>>()?;
        points.reverse();
        let mut contributors = VecDeque::new();
        if version == 3 {
            let mut stmt = tx.prepare("WITH recent AS (
                SELECT seq,elapsed_ms,processes_recorded FROM system_point ORDER BY seq DESC LIMIT ?1
            ) SELECT s.seq,s.elapsed_ms,c.pid,c.creation_ticks,substr(c.name,1,260),c.cpu,c.private_bytes
              FROM recent s LEFT JOIN contributor c ON s.seq=c.seq AND c.rank BETWEEN 0 AND 11
              WHERE s.processes_recorded=1 ORDER BY s.seq,c.rank LIMIT ?2")?;
            let mut rows = stmt.query(params![
                LIVE_CAPACITY as i64,
                (LIVE_CAPACITY * CONTRIBUTOR_LIMIT) as i64
            ])?;
            let mut group = None::<(i64, u64, Vec<Contributor>)>;
            while let Some(row) = rows.next()? {
                let seq: i64 = row.get(0)?;
                let elapsed_ms = u64::try_from(row.get::<_, i64>(1)?)?;
                if group.as_ref().is_none_or(|g| g.0 != seq) {
                    if let Some((_, elapsed_ms, entries)) = group.take() {
                        contributors.push_back(ContributorPoint {
                            elapsed_ms,
                            entries: entries.into(),
                        });
                    }
                    group = Some((seq, elapsed_ms, Vec::new()));
                }
                let entries = &mut group.as_mut().unwrap().2;
                if let Some(pid) = row
                    .get::<_, Option<u32>>(2)?
                    .filter(|_| entries.len() < CONTRIBUTOR_LIMIT)
                {
                    entries.push(Contributor {
                        key: ProcessKey {
                            pid,
                            created_at_ticks: u64::try_from(row.get::<_, i64>(3)?)?,
                        },
                        name: row.get(4)?,
                        cpu_percent: row
                            .get::<_, Option<f64>>(5)?
                            .filter(|v| (0. ..=100.).contains(v)),
                        private_bytes: row
                            .get::<_, Option<i64>>(6)?
                            .map(u64::try_from)
                            .transpose()?,
                    });
                }
            }
            if let Some((_, elapsed_ms, entries)) = group {
                contributors.push_back(ContributorPoint {
                    elapsed_ms,
                    entries: entries.into(),
                });
            }
        }
        (points, contributors)
    };
    tx.commit()?;
    Ok(Replay {
        points: points.into(),
        contributors,
    })
}

const CSV_HEADER: &str = "utc_ms,elapsed_ms,cpu_percent,ram_percent,gpu_percent,disk_bytes_per_second,network_bytes_per_second\r\n";

fn csv_row(w: &mut impl Write, point: Point) -> io::Result<()> {
    write!(w, "{},{}", point.utc_ms, point.elapsed_ms)?;
    for value in point.values.into_iter().chain(point.rates) {
        write!(w, ",")?;
        if let Some(value) = value.filter(|v| v.is_finite()) {
            write!(w, "{value}")?;
        }
    }
    write!(w, "\r\n")
}

pub fn csv_text(points: impl IntoIterator<Item = Point>) -> String {
    let mut out = CSV_HEADER.as_bytes().to_vec();
    for p in points {
        csv_row(&mut out, p).expect("writing into memory");
    }
    String::from_utf8(out).expect("CSV contains only ASCII numeric fields")
}

pub fn export_csv(session: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let (conn, version) = open_session(session)?;
    let mut stmt = conn.prepare(&query(version, "ASC"))?;
    let mut rows = stmt.query([MAX as i64])?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut output = io::BufWriter::new(file);
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        output.write_all(CSV_HEADER.as_bytes())?;
        while let Some(row) = rows.next()? {
            csv_row(&mut output, read_point(row)?)?;
        }
        output.flush()?;
        Ok(())
    })();
    drop(output);
    if result.is_err() {
        let _ = std::fs::remove_file(destination);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contributor_details_follow_retention_and_are_excluded_from_exports() {
        let path = std::env::temp_dir().join(format!(
            "pulse-ranked-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut file = Recording::create(&path, 2).unwrap();
        let ranked = |pid, name: &str| Contributor {
            key: ProcessKey {
                pid,
                created_at_ticks: 77 + u64::from(pid),
            },
            name: name.into(),
            cpu_percent: Some(32.),
            private_bytes: None,
        };
        for (i, entries) in [
            vec![ranked(40, "old-private.exe")],
            vec![ranked(41, "closed-private.exe")],
            vec![],
        ]
        .iter()
        .enumerate()
        {
            file.append_with_contributors(
                Point {
                    utc_ms: i as u64 + 100,
                    elapsed_ms: i as u64 * 1000,
                    values: [Some(50.), None, None],
                    rates: [None; 2],
                },
                Some(entries),
            )
            .unwrap();
        }
        let orphan: i64 = file
            .conn
            .query_row("SELECT COUNT(*) FROM contributor WHERE seq=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(orphan, 0);
        drop(file);
        let loaded = replay_with_contributors(&path).unwrap();
        assert_eq!(loaded.points.len(), 2);
        assert_eq!(loaded.contributors.len(), 2);
        assert_eq!(
            &*loaded.contributors[0].entries,
            &[ranked(41, "closed-private.exe")]
        );
        assert!(loaded.contributors[1].entries.is_empty());
        let csv = path.with_extension("csv");
        export_csv(&path, &csv).unwrap();
        let csv_text = std::fs::read_to_string(&csv).unwrap();
        assert!(!csv_text.contains("private.exe"));
        std::fs::remove_file(csv).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn wal_recording_survives_reopen_with_retention_and_nulls() {
        let path = std::env::temp_dir().join(format!(
            "pulse-history-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let mut file = Recording::create(&path, 3).unwrap();
            for i in 0..5 {
                file.append(Point {
                    utc_ms: 100 - i,
                    elapsed_ms: i * 1000,
                    values: [Some(i as f64), None, None],
                    rates: [Some(2000. + i as f64), None],
                })
                .unwrap();
            }
            assert!(Recording::create(&path, 3).is_err());
        }
        let loaded = replay(&path).unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded.front().unwrap().utc_ms, 98);
        assert_eq!(loaded.back().unwrap().elapsed_ms, 4000);
        assert_eq!(loaded.back().unwrap().values, [Some(4.), None, None]);
        assert_eq!(loaded.back().unwrap().rates, [Some(2004.), None]);
        let csv = path.with_extension("csv");
        export_csv(&path, &csv).unwrap();
        let text = std::fs::read_to_string(&csv).unwrap();
        assert_eq!(text, csv_text(loaded.iter().copied()));
        assert!(text.ends_with("96,4000,4,,,2004,\r\n"));
        assert!(export_csv(&path, &csv).is_err());
        std::fs::remove_file(csv).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn old_sessions_replay_without_inventing_rates() {
        let path = std::env::temp_dir().join(format!(
            "pulse-v1-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch("PRAGMA application_id=1347767379; PRAGMA user_version=1;
              CREATE TABLE system_point(seq INTEGER PRIMARY KEY,utc_ms INTEGER,elapsed_ms INTEGER,cpu REAL,ram REAL,gpu REAL);
              INSERT INTO system_point VALUES (1,702,3,20.0,40.0,NULL);").unwrap();
        }
        let loaded = replay(&path).unwrap();
        assert_eq!(loaded[0].values, [Some(20.), Some(40.), None]);
        assert_eq!(loaded[0].rates, [None, None]);
        std::fs::remove_file(path).unwrap();
    }
}
