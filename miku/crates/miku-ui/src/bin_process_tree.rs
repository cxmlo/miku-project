use std::collections::BTreeSet;
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProcessIdentity {
    pub(crate) pid: u32,
    pub(crate) name_lower: String,
    pub(crate) creation_date: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcessRow {
    pub(crate) pid: u32,
    pub(crate) parent_pid: u32,
    pub(crate) name: String,
    pub(crate) private_bytes: u64,
    pub(crate) creation_date: String,
}

pub(crate) fn compute_percentile(samples: &[f64], pct: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((pct / 100.0) * ((sorted.len() - 1) as f64)).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

pub(crate) fn query_windows_process_snapshot() -> Vec<ProcessRow> {
    #[cfg(target_os = "windows")]
    {
        let ps_script = "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId)`t$($_.ParentProcessId)`t$($_.Name)`t$($_.PrivatePageCount)`t$($_.CreationDate)\" }";
        let output = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", ps_script])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();

        if let Ok(out) = output {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut rows = Vec::new();
            for line in text.lines() {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 5 {
                    if let (Ok(pid), Ok(ppid), Ok(priv_bytes)) = (
                        parts[0].trim().parse::<u32>(),
                        parts[1].trim().parse::<u32>(),
                        parts[3].trim().parse::<u64>(),
                    ) {
                        rows.push(ProcessRow {
                            pid,
                            parent_pid: ppid,
                            name: parts[2].trim().to_string(),
                            private_bytes: priv_bytes,
                            creation_date: parts[4].trim().to_string(),
                        });
                    }
                }
            }
            return rows;
        }
        Vec::new()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

pub(crate) fn collect_descendant_tree(
    root_pid: u32,
    snapshot: &[ProcessRow],
) -> (u64, BTreeSet<ProcessIdentity>) {
    let mut visited_pids = BTreeSet::new();
    let mut queue = vec![root_pid];
    let mut total_private_bytes = 0u64;
    let mut identities = BTreeSet::new();

    while let Some(current_pid) = queue.pop() {
        if !visited_pids.insert(current_pid) {
            continue;
        }
        for row in snapshot {
            if row.pid == current_pid {
                total_private_bytes = total_private_bytes.saturating_add(row.private_bytes);
                identities.insert(ProcessIdentity {
                    pid: row.pid,
                    name_lower: row.name.to_ascii_lowercase(),
                    creation_date: row.creation_date.clone(),
                });
            }
            if row.parent_pid == current_pid && !visited_pids.contains(&row.pid) {
                queue.push(row.pid);
            }
        }
    }

    (total_private_bytes, identities)
}

pub(crate) fn kill_process_tree(pid: u32, child: &mut Child) {
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}
