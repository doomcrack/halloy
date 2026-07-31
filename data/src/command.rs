use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Open (or create) a direct conversation with an address. Without an
    /// address the UI opens the New DM dialog instead.
    Dm(Option<String>),
    /// Create a group conversation with a name and optional description.
    /// Without a name the UI opens the New Group dialog instead.
    Group(Option<String>, Option<String>),
    /// Add a member to the focused group conversation.
    Add(String),
    /// Set a local nickname for the focused conversation. Without an
    /// argument the UI opens the nickname dialog instead.
    Nick(Option<String>),
    /// Toggle the details panel of the focused conversation.
    Details,
    /// Clear the focused buffer's messages.
    Clear,
}

pub fn parse(input: &str) -> Result<Command, Error> {
    let Some(rest) = input.strip_prefix('/') else {
        return Err(Error::MissingSlash);
    };

    if rest.starts_with('/') {
        return Err(Error::HasDoubleSlash);
    }

    let mut split = rest.split_ascii_whitespace();
    let command = split.next().unwrap_or_default().to_lowercase();
    let args = split.collect::<Vec<_>>();

    let arg_count_error = |expected: usize| Error::IncorrectArgCount {
        command: command.clone(),
        expected,
        actual: args.len(),
    };

    match command.as_str() {
        "dm" => match args.as_slice() {
            [] => Ok(Command::Dm(None)),
            [address] => Ok(Command::Dm(Some((*address).to_string()))),
            _ => Err(arg_count_error(1)),
        },
        "group" => match args.split_first() {
            Some((name, description)) => Ok(Command::Group(
                Some((*name).to_string()),
                (!description.is_empty()).then(|| description.join(" ")),
            )),
            None => Ok(Command::Group(None, None)),
        },
        "add" => match args.as_slice() {
            [address] => Ok(Command::Add((*address).to_string())),
            _ => Err(arg_count_error(1)),
        },
        "nick" => {
            if args.is_empty() {
                Ok(Command::Nick(None))
            } else {
                Ok(Command::Nick(Some(args.join(" "))))
            }
        }
        "details" => {
            if args.is_empty() {
                Ok(Command::Details)
            } else {
                Err(arg_count_error(0))
            }
        }
        "clear" => {
            if args.is_empty() {
                Ok(Command::Clear)
            } else {
                Err(arg_count_error(0))
            }
        }
        _ => Err(Error::Unknown(command)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("input is not a command")]
    MissingSlash,
    #[error("input starts with a literal slash")]
    HasDoubleSlash,
    #[error("unknown command: /{0}")]
    Unknown(String),
    #[error("/{command} expects {expected} argument(s), got {actual}")]
    IncorrectArgCount {
        command: String,
        expected: usize,
        actual: usize,
    },
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Command::Dm(Some(address)) => write!(f, "/dm {address}"),
            Command::Dm(None) => write!(f, "/dm"),
            Command::Group(Some(name), Some(description)) => {
                write!(f, "/group {name} {description}")
            }
            Command::Group(Some(name), None) => write!(f, "/group {name}"),
            Command::Group(None, _) => write!(f, "/group"),
            Command::Add(address) => write!(f, "/add {address}"),
            Command::Nick(Some(nickname)) => write!(f, "/nick {nickname}"),
            Command::Nick(None) => write!(f, "/nick"),
            Command::Details => write!(f, "/details"),
            Command::Clear => write!(f, "/clear"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_command_set() {
        assert_eq!(
            parse("/dm deadbeef"),
            Ok(Command::Dm(Some("deadbeef".to_string())))
        );
        assert_eq!(
            parse("/group pals"),
            Ok(Command::Group(Some("pals".to_string()), None))
        );
        assert_eq!(
            parse("/group pals a place for pals"),
            Ok(Command::Group(
                Some("pals".to_string()),
                Some("a place for pals".to_string())
            ))
        );
        assert_eq!(
            parse("/add cafef00d"),
            Ok(Command::Add("cafef00d".to_string()))
        );
        assert_eq!(
            parse("/nick my friend"),
            Ok(Command::Nick(Some("my friend".to_string())))
        );
        assert_eq!(parse("/details"), Ok(Command::Details));
        assert_eq!(parse("/CLEAR"), Ok(Command::Clear));
    }

    #[test]
    fn missing_args_open_the_matching_dialog() {
        assert_eq!(parse("/dm"), Ok(Command::Dm(None)));
        assert_eq!(parse("/group"), Ok(Command::Group(None, None)));
        assert_eq!(parse("/nick"), Ok(Command::Nick(None)));
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(parse("hello"), Err(Error::MissingSlash));
        assert_eq!(parse("//literal"), Err(Error::HasDoubleSlash));
        assert_eq!(parse("/wave"), Err(Error::Unknown("wave".to_string())));
        assert_eq!(
            parse("/dm too many"),
            Err(Error::IncorrectArgCount {
                command: "dm".to_string(),
                expected: 1,
                actual: 2,
            })
        );
        assert_eq!(
            parse("/clear now"),
            Err(Error::IncorrectArgCount {
                command: "clear".to_string(),
                expected: 0,
                actual: 1,
            })
        );
    }
}
