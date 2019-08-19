use auto_enums::auto_enum;
use glob::{glob_with, MatchOptions};
use ion_shell::{expansion::Expander, Shell};
use itertools::Itertools;
use liner::{Completer, CursorPosition, Event, EventKind};
use shellac::{
    codec::{read_reply, write_request, AutocompRequest},
    Error,
};
use std::{
    env,
    io::BufReader,
    iter,
    path::PathBuf,
    process::{Command, Stdio},
    str,
};

pub struct IonCompleter<'a, 'b> {
    shell:      &'b Shell<'a>,
    completion: CompletionType,
}

/// Unescape filenames for the completer so that special characters will be properly shown.
fn unescape(input: &str) -> String {
    let mut output = Vec::with_capacity(input.len());
    let mut check = false;
    for character in input.bytes() {
        match character {
            b'\\' if !check => check = true,
            b'(' | b')' | b'[' | b']' | b'&' | b'$' | b'@' | b'{' | b'}' | b'<' | b'>' | b';'
            | b'"' | b'\'' | b'#' | b'^' | b'*' | b' '
                if check =>
            {
                output.push(character);
                check = false;
            }
            _ if check => {
                output.extend(&[b'\\', character]);
                check = false;
            }
            _ => output.push(character),
        }
    }
    unsafe { String::from_utf8_unchecked(output) }
}

/// Escapes filenames from the completer so that special characters will be properly escaped.
///
/// NOTE: Perhaps we should submit a PR to Liner to add a &'static [u8] field to
/// `FilenameCompleter` so that we don't have to perform the escaping ourselves?
fn escape(input: &str) -> String {
    let mut output = Vec::with_capacity(input.len());
    for character in input.bytes() {
        match character {
            b'(' | b')' | b'[' | b']' | b'&' | b'$' | b'@' | b'{' | b'}' | b'<' | b'>' | b';'
            | b'"' | b'\'' | b'#' | b'^' | b'*' | b' ' => output.push(b'\\'),
            _ => (),
        }
        output.push(character);
    }
    unsafe { String::from_utf8_unchecked(output) }
}

enum CompletionType {
    Nothing,
    Command,
    VariableAndFiles(AutocompRequest),
}

impl<'a, 'b> IonCompleter<'a, 'b> {
    pub fn new(shell: &'b Shell<'a>) -> Self {
        IonCompleter { shell, completion: CompletionType::Nothing }
    }
}

impl<'a, 'b> Completer for IonCompleter<'a, 'b> {
    fn completions(&mut self, start: &str) -> Vec<String> {
        let mut completions = Vec::with_capacity(20);
        let vars = self.shell.variables();

        match &self.completion {
            CompletionType::VariableAndFiles(request) => {
                // Initialize a new completer from the definitions collected.
                // Creates a list of definitions from the shell environment that
                // will be used
                // in the creation of a custom completer.
                if start.starts_with('$') {
                    completions.extend(
                        // Add the list of available variables to the completer's
                        // definitions. TODO: We should make
                        // it free to do String->SmallString
                        //       and mostly free to go back (free if allocated)
                        vars.string_vars()
                            .filter(|(s, _)| s.starts_with(&start[1..]))
                            .map(|(s, _)| format!("${}", &s)),
                    );
                } else if start.starts_with('@') {
                    completions.extend(
                        vars.arrays()
                            .filter(|(s, _)| s.starts_with(&start[1..]))
                            .map(|(s, _)| format!("@{}", &s)),
                    );
                } else {
                    let mut cmd =
                        Command::new("/home/adminxvii/dev/shellac-server/target/debug/shellac");
                    cmd.current_dir("/home/adminxvii/dev/shellac-server");
                    let child = cmd
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::null())
                        .spawn()
                        .expect("Failed to spawn child process");

                    {
                        let mut stdin = child.stdin.expect("Failed to open stdin");
                        write_request(&mut stdin, request);
                    }

                    let output = read_reply(
                        &mut BufReader::new(child.stdout.unwrap()),
                        |suggestions| -> Result<_, Error> {
                            suggestions
                                .map(|s| s.map(|(s, _)| format!("{}{}", start, s)))
                                .collect::<Result<Vec<_>, _>>()
                        },
                    );
                    if let Ok(output) = output {
                        completions.extend(output)
                    }
                    // TODO: This is really dirty and only works for completely typed command. Fix
                    // this
                }
            }
            CompletionType::Command => {
                // Initialize a new completer from the definitions collected.
                // Creates a list of definitions from the shell environment that
                // will be used
                // in the creation of a custom completer.
                completions.extend(IonFileCompleter::new(None, &self.shell).completions(start));
                completions.extend(
                    self.shell
                        .builtins()
                        .keys()
                        // Add built-in commands to the completer's definitions.
                        .map(ToString::to_string)
                        // Add the aliases to the completer's definitions.
                        .chain(vars.aliases().map(|(key, _)| key.to_string()))
                        // Add the list of available functions to the completer's
                        // definitions.
                        .chain(vars.functions().map(|(key, _)| key.to_string()))
                        .filter(|s| s.starts_with(start)),
                );
                // Creates completers containing definitions from all directories
                // listed
                // in the environment's **$PATH** variable.
                let file_completers: Vec<_> = if let Some(paths) = env::var_os("PATH") {
                    env::split_paths(&paths)
                        .map(|s| {
                            let s = if !s.to_string_lossy().ends_with('/') {
                                let mut oss = s.into_os_string();
                                oss.push("/");
                                oss.into()
                            } else {
                                s
                            };
                            IonFileCompleter::new(Some(s), &self.shell)
                        })
                        .collect()
                } else {
                    vec![IonFileCompleter::new(Some("/bin/".into()), &self.shell)]
                };
                // Merge the collected definitions with the file path definitions.
                completions.extend(MultiCompleter::new(file_completers).completions(start));
            }
            CompletionType::Nothing => {
                completions.extend(IonFileCompleter::new(None, &self.shell).completions(start))
            }
        }

        completions
    }

    fn on_event<W: std::io::Write>(&mut self, event: Event<'_, '_, W>) {
        if let EventKind::BeforeComplete = event.kind {
            let (words, pos) = event.editor.get_words_and_cursor_position();
            let variables = |index, append| {
                // Find the incomplete statement
                // TODO: proper expansion
                let initial_len = words.len();
                let buffer = event.editor.current_buffer();
                let mut words = words
                    .iter()
                    .rev()
                    .map(|&(start, end)| buffer.range(start, end))
                    .take_while(|word| {
                        !word.ends_with('|') && !word.ends_with('&') && !word.ends_with(';')
                    })
                    .collect::<Vec<_>>();
                words.reverse();
                if append {
                    words.push(String::new());
                }

                let len_diff = initial_len + if append { 1 } else { 0 } - words.len();
                AutocompRequest { argv: words, word: (index - len_diff) as u16 }
            };

            self.completion = match pos {
                _ if words.is_empty() => CompletionType::Nothing,
                CursorPosition::InWord(0)
                | CursorPosition::OnWordRightEdge(0)
                | CursorPosition::InSpace(None, _) => CompletionType::Command,
                CursorPosition::OnWordRightEdge(index) => {
                    let is_pipe = words
                        .iter()
                        .nth(index - 1)
                        .map(|&(start, end)| event.editor.current_buffer().range(start, end))
                        .map_or(false, |filename| {
                            filename.ends_with('|')
                                || filename.ends_with('&')
                                || filename.ends_with(';')
                        });
                    if is_pipe {
                        CompletionType::Command
                    } else {
                        CompletionType::VariableAndFiles(variables(index, false))
                    }
                }
                CursorPosition::InWord(index)
                | CursorPosition::OnWordLeftEdge(index)
                | CursorPosition::InSpace(_, Some(index)) => {
                    CompletionType::VariableAndFiles(variables(index, false))
                }
                CursorPosition::InSpace(Some(index), None) => {
                    CompletionType::VariableAndFiles(variables(index + 1, true))
                }
            };
        }
    }
}

/// Performs escaping to an inner `FilenameCompleter` to enable a handful of special cases
/// needed by the shell, such as expanding '~' to a home directory, or adding a backslash
/// when a special character is contained within an expanded filename.
pub struct IonFileCompleter<'a, 'b> {
    shell:       &'b Shell<'a>,
    /// The directory the expansion takes place in
    path:        PathBuf,
    for_command: bool,
}

impl<'a, 'b> IonFileCompleter<'a, 'b> {
    pub fn new(path: Option<PathBuf>, shell: &'b Shell<'a>) -> Self {
        // The only time a path is Some is when looking for a command not a directory
        // so save this fact to strip the paths when completing commands.
        let for_command = path.is_some();
        let path = path.unwrap_or_default();
        IonFileCompleter { shell, path, for_command }
    }
}

impl<'a, 'b> Completer for IonFileCompleter<'a, 'b> {
    /// When the tab key is pressed, **Liner** will use this method to perform completions of
    /// filenames. As our `IonFileCompleter` is a wrapper around **Liner**'s
    /// `FilenameCompleter`,
    /// the purpose of our custom `Completer` is to expand possible `~` characters in the
    /// `start`
    /// value that we receive from the prompt, grab completions from the inner
    /// `FilenameCompleter`,
    /// and then escape the resulting filenames, as well as remove the expanded form of the `~`
    /// character and re-add the `~` character in it's place.
    fn completions(&mut self, start: &str) -> Vec<String> {
        // Dereferencing the raw pointers here should be entirely safe, theoretically,
        // because no changes will occur to either of the underlying references in the
        // duration between creation of the completers and execution of their
        // completions.
        let expanded = match self.shell.tilde(start) {
            Ok(expanded) => expanded,
            Err(why) => {
                eprintln!("ion: {}", why);
                return vec![start.into()];
            }
        };
        // Now we obtain completions for the `expanded` form of the `start` value.
        let completions = filename_completion(&expanded, &self.path);
        if expanded == start {
            return if self.for_command {
                completions
                    .map(|s| s.rsplit('/').next().map(|s| s.to_string()).unwrap_or(s))
                    .collect()
            } else {
                completions.collect()
            };
        }
        // We can do that by obtaining the index position where the tilde character
        // ends. We don't search with `~` because we also want to
        // handle other tilde variants.
        let t_index = start.find('/').unwrap_or(1);
        // `tilde` is the tilde pattern, and `search` is the pattern that follows.
        let (tilde, search) = start.split_at(t_index);

        if search.len() < 2 {
            // If the length of the search pattern is less than 2, the search pattern is
            // empty, and thus the completions actually contain files and directories in
            // the home directory.

            // The tilde pattern will actually be our `start` command in itself,
            // and the completed form will be all of the characters beyond the length of
            // the expanded form of the tilde pattern.
            completions.map(|completion| [start, &completion[expanded.len()..]].concat()).collect()
        // To save processing time, we should get obtain the index position where our
        // search pattern begins, and re-use that index to slice the completions so
        // that we may re-add the tilde character with the completion that follows.
        } else if let Some(e_index) = expanded.rfind(search) {
            // And then we will need to take those completions and remove the expanded form
            // of the tilde pattern and replace it with that pattern yet again.
            completions.map(|completion| [tilde, &completion[e_index..]].concat()).collect()
        } else {
            Vec::new()
        }
    }
}

#[auto_enum]
fn filename_completion<'a>(start: &'a str, path: &'a PathBuf) -> impl Iterator<Item = String> + 'a {
    let unescaped_start = unescape(start);

    let mut split_start = unescaped_start.split('/');
    let mut string = String::with_capacity(128);

    // When 'start' is an absolute path, "/..." gets split to ["", "..."]
    // So we skip the first element and add "/" to the start of the string
    if unescaped_start.starts_with('/') {
        split_start.next();
        string.push('/');
    } else {
        string.push_str(&path.to_string_lossy());
    }

    for element in split_start {
        string.push_str(element);
        if element != "." && element != ".." {
            string.push('*');
        }
        string.push('/');
    }

    string.pop(); // pop out the last '/' character
    if string.ends_with('.') {
        string.push('*')
    }
    let globs = glob_with(
        &string,
        MatchOptions {
            case_sensitive:              true,
            require_literal_separator:   true,
            require_literal_leading_dot: false,
        },
    )
    .ok()
    .map(|completions| {
        completions.filter_map(Result::ok).filter_map(move |file| {
            let out = file.to_str()?;
            let mut joined = String::with_capacity(out.len() + 3); // worst case senario
            if unescaped_start.starts_with("./") {
                joined.push_str("./");
            }
            joined.push_str(out);
            if file.is_dir() {
                joined.push('/');
            }
            Some(escape(&joined))
        })
    });

    #[auto_enum(Iterator)]
    match globs {
        Some(iter) => iter,
        None => iter::once(start.into()),
    }
}

/// A completer that combines suggestions from multiple completers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MultiCompleter<A>(Vec<A>);

impl<A> MultiCompleter<A> {
    pub fn new(completions: Vec<A>) -> Self { MultiCompleter(completions) }
}

impl<A> Completer for MultiCompleter<A>
where
    A: Completer,
{
    fn completions(&mut self, start: &str) -> Vec<String> {
        self.0.iter_mut().flat_map(|comp| comp.completions(start)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_completion() {
        let shell = Shell::default();
        let mut completer = IonFileCompleter::new(None, &shell);
        assert_eq!(completer.completions("testing"), vec!["testing/"]);
        assert_eq!(completer.completions("testing/file"), vec!["testing/file_with_text"]);
        if cfg!(not(target_os = "redox")) {
            assert_eq!(completer.completions("~"), vec!["~/"]);
        }
        assert_eq!(completer.completions("tes/fil"), vec!["testing/file_with_text"]);
    }
}
