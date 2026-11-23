use super::{Point, Recording, Replay, replay_with_contributors};
use pulse_domain::Contributor;
use std::path::PathBuf;

#[derive(Debug)]
pub enum SessionCommand {
    Start {
        path: PathBuf,
        retain: usize,
        include_processes: bool,
    },
    Stop,
    Open(PathBuf),
}

#[derive(Debug, Clone)]
pub struct SessionReply {
    pub recording: bool,
    pub message: String,
    pub replay: Option<Replay>,
}

#[derive(Default)]
pub struct SessionWriter {
    recorder: Option<Recording>,
    origin_ms: Option<u64>,
    include_processes: bool,
}

impl SessionWriter {
    pub fn apply(&mut self, command: SessionCommand) -> SessionReply {
        let mut replayed = None;
        let message = match command {
            SessionCommand::Start { .. } if self.recorder.is_some() => {
                "Un enregistrement est déjà actif.".into()
            }
            SessionCommand::Start {
                path,
                retain,
                include_processes,
            } => match Recording::create(&path, retain) {
                Ok(recorder) => {
                    self.recorder = Some(recorder);
                    self.origin_ms = None;
                    self.include_processes = include_processes;
                    if include_processes {
                        "Enregistrement SQLite actif · noms de processus inclus.".into()
                    } else {
                        "Enregistrement SQLite actif · métriques système seules.".into()
                    }
                }
                Err(e) => format!("Impossible de créer la session : {e}"),
            },
            SessionCommand::Stop => {
                self.recorder.take();
                self.origin_ms = None;
                "Enregistrement arrêté. Le fichier est prêt à être relu.".into()
            }
            SessionCommand::Open(_) if self.recorder.is_some() => {
                "Arrêtez l’enregistrement avant d’ouvrir une session.".into()
            }
            SessionCommand::Open(path) => match replay_with_contributors(&path) {
                Ok(points) => {
                    replayed = Some(points);
                    "Session ouverte en lecture seule.".into()
                }
                Err(e) => format!("Impossible de lire la session : {e}"),
            },
        };
        SessionReply {
            recording: self.recorder.is_some(),
            message,
            replay: replayed,
        }
    }

    pub fn append(&mut self, point: Point) -> Option<SessionReply> {
        self.append_with_contributors(point, None)
    }

    pub fn append_with_contributors(
        &mut self,
        mut point: Point,
        contributors: Option<&[Contributor]>,
    ) -> Option<SessionReply> {
        let file = self.recorder.as_mut()?;
        let origin = *self.origin_ms.get_or_insert(point.elapsed_ms);
        point.elapsed_ms = point.elapsed_ms.saturating_sub(origin);
        if let Err(e) =
            file.append_with_contributors(point, contributors.filter(|_| self.include_processes))
        {
            self.recorder = None;
            return Some(SessionReply {
                recording: false,
                message: format!("Enregistrement arrêté : {e}"),
                replay: None,
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_stops_reopens_and_keeps_each_session_clock_independent() {
        let path = std::env::temp_dir().join(format!(
            "pulse-session-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut writer = SessionWriter::default();
        assert!(
            writer
                .apply(SessionCommand::Start {
                    path: path.clone(),
                    retain: 2,
                    include_processes: false
                })
                .recording
        );
        for elapsed_ms in [8000, 9000, 10000] {
            assert!(
                writer
                    .append(Point {
                        utc_ms: 100 + elapsed_ms,
                        elapsed_ms,
                        values: [Some(37.), None, None],
                        rates: [None, Some(5.)]
                    })
                    .is_none()
            );
        }
        let refused = writer.apply(SessionCommand::Open(path.clone()));
        assert!(refused.recording);
        assert!(refused.replay.is_none());
        assert!(!writer.apply(SessionCommand::Stop).recording);
        let opened = writer
            .apply(SessionCommand::Open(path.clone()))
            .replay
            .unwrap();
        assert_eq!(opened.points.len(), 2);
        assert_eq!(opened.points[0].elapsed_ms, 1000);
        assert_eq!(opened.points[1].rates[1], Some(5.));
        assert!(opened.contributors.is_empty());
        assert!(
            !writer
                .apply(SessionCommand::Start {
                    path: path.clone(),
                    retain: 2,
                    include_processes: false
                })
                .recording
        );
        std::fs::remove_file(path).unwrap();
    }
}
