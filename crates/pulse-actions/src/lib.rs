#![forbid(unsafe_code)]
use pulse_domain::{Priority, ProcessKey};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorityTarget {
    Idle,
    Low,
    Normal,
    AboveNormal,
}

impl PriorityTarget {
    pub fn priority(self) -> Priority {
        match self {
            Self::Idle => Priority::Idle,
            Self::Low => Priority::Low,
            Self::Normal => Priority::Normal,
            Self::AboveNormal => Priority::AboveNormal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Terminate,
    SetPriority(PriorityTarget),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command {
    pub key: ProcessKey,
    pub change: Change,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Requested,
    PriorityApplied,
    Protected,
    InstanceChanged,
    OsError(i32),
}

impl Outcome {
    pub fn message(self) -> String {
        match self {
            Self::Requested => "Windows a accepté la demande de terminaison.".into(),
            Self::PriorityApplied => "Windows a accepté le changement de priorité.".into(),
            Self::Protected => "Action refusée : processus protégé ou critique.".into(),
            Self::InstanceChanged => "Cette instance du processus n’est plus active.".into(),
            Self::OsError(code) => format!("Windows a refusé la demande ({code:#010x})."),
        }
    }
}

pub fn channel() -> (SyncSender<Command>, Receiver<Command>) {
    sync_channel(1)
}
