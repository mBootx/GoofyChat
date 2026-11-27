//! "@username" mentions in message bodies.

use proto::limits;

fn name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.'
}

/// The usernames mentioned in a body, lowercase, deduplicated, in order of appearance.
///
/// A mention is an `@` at the start or after a character that is not `[A-Za-z0-9_.@]`,
/// followed by 3 to 24 characters of `[A-Za-z0-9_.]` (trailing dots are not part of it).
pub fn parse(body: &str) -> Vec<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut found: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '@' || (i > 0 && (name_char(chars[i - 1]) || chars[i - 1] == '@')) {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut end = start;
        while end < chars.len() && name_char(chars[end]) {
            end += 1;
        }
        let mut name: String = chars[start..end].iter().collect();
        while name.ends_with('.') {
            name.pop();
        }
        if (3..=24).contains(&name.len()) {
            let name = name.to_ascii_lowercase();
            if !found.contains(&name) {
                found.push(name);
            }
        }
        i = end.max(start);
    }
    found
}

/// The ids of the members (other than the sender) mentioned in a body, at most
/// [`limits::MENTIONS`], in order of appearance.
pub fn resolve(body: &str, members: &[(i64, String)], sender: i64) -> Vec<i64> {
    parse(body)
        .iter()
        .filter_map(|name| members.iter().find(|(id, u)| *id != sender && u == name).map(|m| m.0))
        .take(limits::MENTIONS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mentions() {
        assert_eq!(parse("@bob salut"), ["bob"]);
        assert_eq!(parse("salut @Bob, @alice. et @bob"), ["bob", "alice"]);
        assert_eq!(parse("(@carole)"), ["carole"]);
        assert_eq!(parse("a.b@c.com moi@bob"), Vec::<String>::new(), "e-mail addresses are not mentions");
        assert_eq!(parse("@@bob"), Vec::<String>::new());
        assert_eq!(parse("@bo @x"), Vec::<String>::new(), "too short");
        assert_eq!(parse(&format!("@{}", "a".repeat(25))), Vec::<String>::new(), "too long");
        assert_eq!(parse(&format!("@{}...", "a".repeat(24))), ["a".repeat(24)]);
        assert_eq!(parse("@jean.paul_2..."), ["jean.paul_2"]);
        assert_eq!(parse("é@bob"), ["bob"], "any other character separates");
        assert_eq!(parse("@"), Vec::<String>::new());
    }

    #[test]
    fn resolves_members_only() {
        let members = vec![(1, "alice".to_string()), (2, "bob".to_string())];
        assert_eq!(resolve("@alice @bob @eve", &members, 1), [2]);
        assert_eq!(resolve("@bob @alice", &members, 3), [2, 1]);
        let many: Vec<(i64, String)> = (0..30).map(|i| (i, format!("user{i:02}"))).collect();
        let body: String = (0..30).map(|i| format!("@user{i:02} ")).collect();
        assert_eq!(resolve(&body, &many, 99).len(), limits::MENTIONS);
    }
}
