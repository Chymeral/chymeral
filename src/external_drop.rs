use std::{
    error::Error,
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
};

pub const MAX_EXTERNAL_DROP_FILES: usize = 128;
pub const MAX_EXTERNAL_DROP_URI_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalFile {
    path: PathBuf,
    uri: String,
}

impl ExternalFile {
    pub fn from_path(path: PathBuf) -> Result<Self, ExternalDropError> {
        if !path.is_absolute() {
            return Err(ExternalDropError::RelativePath);
        }
        let uri = file_uri(&path)?;
        Ok(Self { path, uri })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalQuoting {
    Posix,
    Cmd,
}

impl TerminalQuoting {
    pub const fn platform_default() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::Cmd
        }
        #[cfg(not(target_os = "windows"))]
        {
            Self::Posix
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalDropError {
    RelativePath,
    UriLimitExceeded { limit: usize },
    BatchLimitExceeded { limit: usize },
    EmptyBatch,
    TerminalPathNotUtf8,
    TerminalControlCharacter,
}

impl Display for ExternalDropError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RelativePath => formatter.write_str("a dropped path must be absolute"),
            Self::UriLimitExceeded { limit } => {
                write!(formatter, "dropped file URIs may not exceed {limit} bytes")
            }
            Self::BatchLimitExceeded { limit } => {
                write!(formatter, "a drop may not contain more than {limit} files")
            }
            Self::EmptyBatch => formatter.write_str("a drop must contain at least one file"),
            Self::TerminalPathNotUtf8 => formatter
                .write_str("the dropped path cannot be represented as UTF-8 terminal input"),
            Self::TerminalControlCharacter => formatter.write_str(
                "the dropped path contains a terminal control character and cannot be inserted",
            ),
        }
    }
}

impl Error for ExternalDropError {}

pub fn format_terminal_paths(
    files: &[ExternalFile],
    quoting: TerminalQuoting,
) -> Result<String, ExternalDropError> {
    if files.is_empty() {
        return Err(ExternalDropError::EmptyBatch);
    }
    if files.len() > MAX_EXTERNAL_DROP_FILES {
        return Err(ExternalDropError::BatchLimitExceeded {
            limit: MAX_EXTERNAL_DROP_FILES,
        });
    }

    files
        .iter()
        .map(|file| quote_terminal_path(file.path(), quoting))
        .collect::<Result<Vec<_>, _>>()
        .map(|paths| paths.join(" "))
}

fn file_uri(path: &Path) -> Result<String, ExternalDropError> {
    #[cfg(target_os = "windows")]
    let uri = {
        let path = path
            .to_str()
            .ok_or(ExternalDropError::TerminalPathNotUtf8)?
            .replace('\\', "/");
        if path.starts_with("//") {
            format!("file:{}", percent_encode(path.as_bytes()))
        } else if path.starts_with('/') {
            format!("file://{}", percent_encode(path.as_bytes()))
        } else {
            format!("file:///{}", percent_encode(path.as_bytes()))
        }
    };

    #[cfg(not(target_os = "windows"))]
    let uri = {
        use std::os::unix::ffi::OsStrExt;

        format!("file://{}", percent_encode(path.as_os_str().as_bytes()))
    };

    if uri.len() > MAX_EXTERNAL_DROP_URI_BYTES {
        return Err(ExternalDropError::UriLimitExceeded {
            limit: MAX_EXTERNAL_DROP_URI_BYTES,
        });
    }
    Ok(uri)
}

fn quote_terminal_path(path: &Path, quoting: TerminalQuoting) -> Result<String, ExternalDropError> {
    let path = path
        .to_str()
        .ok_or(ExternalDropError::TerminalPathNotUtf8)?;
    if path.chars().any(char::is_control) {
        return Err(ExternalDropError::TerminalControlCharacter);
    }

    Ok(match quoting {
        TerminalQuoting::Posix => format!("'{}'", path.replace('\'', "'\"'\"'")),
        TerminalQuoting::Cmd => quote_cmd_path(path),
    })
}

fn quote_cmd_path(path: &str) -> String {
    let mut quoted = String::with_capacity(path.len() + 2);
    quoted.push('"');
    for character in path.chars() {
        match character {
            '^' => quoted.push_str("^^"),
            '&' | '|' | '<' | '>' | '(' | ')' | '%' => {
                quoted.push('^');
                quoted.push(character);
            }
            '!' => quoted.push_str("^^^!"),
            '"' => quoted.push_str("^\""),
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

fn percent_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;

            write!(encoded, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[cfg(unix)]
    use std::ffi::OsString;

    use super::{
        ExternalDropError, ExternalFile, TerminalQuoting, format_terminal_paths, percent_encode,
    };

    #[test]
    fn encodes_an_absolute_path_as_a_file_uri() {
        let file = ExternalFile::from_path(absolute_test_path("space # file.txt")).unwrap();

        assert!(file.uri().starts_with("file://"));
        assert!(file.uri().ends_with("space%20%23%20file.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn preserves_non_utf8_posix_path_bytes_in_the_uri() {
        use std::os::unix::ffi::OsStringExt;

        let path = PathBuf::from(OsString::from_vec(b"/tmp/invalid-\xff".to_vec()));
        let file = ExternalFile::from_path(path).unwrap();

        assert_eq!(file.uri(), "file:///tmp/invalid-%FF");
    }

    #[test]
    fn rejects_relative_paths_and_terminal_control_characters() {
        assert_eq!(
            ExternalFile::from_path(PathBuf::from("relative.txt")).unwrap_err(),
            ExternalDropError::RelativePath
        );
        let file = ExternalFile::from_path(absolute_test_path("line\nbreak")).unwrap();
        assert_eq!(
            format_terminal_paths(&[file], TerminalQuoting::Posix),
            Err(ExternalDropError::TerminalControlCharacter)
        );
    }

    #[test]
    fn quotes_paths_for_posix_and_cmd_without_executing_them() {
        let first_path = absolute_test_path("one file's name");
        let second_path = absolute_test_path("two");
        let first = first_path.to_str().unwrap();
        let second = second_path.to_str().unwrap();

        assert_eq!(
            format_terminal_paths(
                &[
                    ExternalFile::from_path(PathBuf::from(first)).unwrap(),
                    ExternalFile::from_path(PathBuf::from(second)).unwrap(),
                ],
                TerminalQuoting::Posix,
            ),
            Ok(format!(
                "'{}' '{}'",
                first.replace('\'', "'\"'\"'"),
                second.replace('\'', "'\"'\"'")
            ))
        );
        assert_eq!(
            format_terminal_paths(
                &[ExternalFile::from_path(PathBuf::from(first)).unwrap()],
                TerminalQuoting::Cmd,
            ),
            Ok(format!("\"{}\"", first))
        );
    }

    #[test]
    fn preserves_unicode_and_quotes_in_a_safe_multi_file_terminal_batch() {
        let files = [
            ExternalFile::from_path(absolute_test_path("árbol \"azul\".txt")).unwrap(),
            ExternalFile::from_path(absolute_test_path("第二 archivo.txt")).unwrap(),
        ];
        let first = files[0].path().to_str().unwrap();
        let second = files[1].path().to_str().unwrap();

        assert_eq!(
            format_terminal_paths(&files, TerminalQuoting::Posix),
            Ok(format!("'{first}' '{second}'"))
        );
        assert_eq!(
            format_terminal_paths(&files, TerminalQuoting::Cmd),
            Ok(format!(
                "\"{}\" \"{}\"",
                first.replace('\"', "^\""),
                second.replace('\"', "^\"")
            ))
        );
    }

    #[test]
    fn rejects_the_entire_terminal_batch_when_any_path_is_unsafe() {
        let safe = ExternalFile::from_path(absolute_test_path("safe.txt")).unwrap();
        let unsafe_path = ExternalFile::from_path(absolute_test_path("line\nbreak.txt")).unwrap();

        assert_eq!(
            format_terminal_paths(&[safe, unsafe_path], TerminalQuoting::Posix),
            Err(ExternalDropError::TerminalControlCharacter)
        );
    }

    #[test]
    fn rejects_batches_larger_than_the_documented_limit() {
        let file = ExternalFile::from_path(absolute_test_path("file.txt")).unwrap();
        let files = vec![file; super::MAX_EXTERNAL_DROP_FILES + 1];

        assert_eq!(
            format_terminal_paths(&files, TerminalQuoting::Posix),
            Err(ExternalDropError::BatchLimitExceeded {
                limit: super::MAX_EXTERNAL_DROP_FILES,
            })
        );
    }

    #[test]
    fn percent_encoding_keeps_file_uri_path_delimiters() {
        assert_eq!(percent_encode(b"/tmp/a:b?c"), "/tmp/a:b%3Fc");
    }

    #[cfg(windows)]
    fn absolute_test_path(name: &str) -> PathBuf {
        PathBuf::from(r"C:\chymeral-test").join(name)
    }

    #[cfg(not(windows))]
    fn absolute_test_path(name: &str) -> PathBuf {
        PathBuf::from("/tmp/chymeral-test").join(name)
    }
}
