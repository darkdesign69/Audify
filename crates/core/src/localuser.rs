//! The Mac user's first name, for "Made for <name>" on Made for you pages (stream: parity3).
//! Local only: read from the account's full name (`id -F`), never sent anywhere.

/// The first word of a full name ("Sam Smith" → "Sam"); `None` for an empty one.
pub fn first_name(full: &str) -> Option<String> {
    full.split_whitespace().next().map(str::to_string)
}

pub fn local_user_name() -> Option<String> {
    let out = std::process::Command::new("/usr/bin/id").arg("-F").output().ok()?;
    if !out.status.success() {
        return None;
    }
    first_name(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::first_name;

    #[test]
    fn takes_the_first_word() {
        assert_eq!(first_name("Sam Smith\n").as_deref(), Some("Sam"));
        assert_eq!(first_name("  Prince ").as_deref(), Some("Prince"));
        assert_eq!(first_name(" \n"), None);
    }
}
