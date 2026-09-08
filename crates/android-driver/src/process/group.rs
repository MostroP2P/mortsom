//! Linux process-group quiescence while the owned leader remains unreaped.
use std::io;

pub(super) fn has_running_members(group: u32) -> io::Result<bool> {
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
            .is_none()
        {
            continue;
        }
        let stat = match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(cause) if cause.kind() == io::ErrorKind::NotFound => continue,
            Err(cause) => return Err(cause),
        };
        if running_member(&stat, group)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn running_member(stat: &str, group: u32) -> io::Result<bool> {
    let (_, fields) = stat
        .rsplit_once(") ")
        .ok_or_else(|| io::Error::other("invalid process stat"))?;
    let mut fields = fields.split_whitespace();
    let state = fields
        .next()
        .ok_or_else(|| io::Error::other("missing process state"))?;
    let pgrp = fields
        .nth(1)
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| io::Error::other("missing process group"))?;
    Ok(pgrp == group && !matches!(state, "Z" | "X"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn names_with_spaces_and_parentheses_do_not_change_group_or_state_parsing() {
        assert!(running_member("12 (name ) with spaces) S 1 42 42", 42).unwrap());
        assert!(!running_member("12 (name) Z 1 42 42", 42).unwrap());
        assert!(!running_member("12 (name) S 1 43 43", 42).unwrap());
        assert!(running_member("malformed", 42).is_err());
    }
}
