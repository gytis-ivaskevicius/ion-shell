use super::{colors::Colors, flow_control::Function};
use crate::{
    expansion::ExpansionError,
    shell::IonError,
    sys::{env as sys_env, geteuid, getpid, getuid, variables as self_sys},
    types::{self, Array},
};
use scopes::{Namespace, Scope, Scopes};
use std::env;
use types_rs::array;
use unicode_segmentation::UnicodeSegmentation;
use xdg::BaseDirectories;

pub use types_rs::Value;
pub struct Variables<'a>(Scopes<types::Str, Value<Function<'a>>>);

impl<'a> Variables<'a> {
    pub fn string_vars(&self) -> impl Iterator<Item = (&types::Str, &types::Str)> {
        self.0.scopes().flat_map(|map| {
            map.iter().filter_map(|(key, val)| {
                if let types_rs::Value::Str(val) = val {
                    Some((key, val))
                } else {
                    None
                }
            })
        })
    }

    pub fn aliases(&self) -> impl Iterator<Item = (&types::Str, &types::Str)> {
        self.0.scopes().rev().flat_map(|map| {
            map.iter().filter_map(|(key, possible_alias)| {
                if let types_rs::Value::Alias(alias) = possible_alias {
                    Some((key, &**alias))
                } else {
                    None
                }
            })
        })
    }

    pub fn functions(&self) -> impl Iterator<Item = (&types::Str, &Function<'a>)> {
        self.0.scopes().rev().flat_map(|map| {
            map.iter().filter_map(|(key, val)| {
                if let types_rs::Value::Function(val) = val {
                    Some((key, val))
                } else {
                    None
                }
            })
        })
    }

    pub fn arrays(&self) -> impl Iterator<Item = (&types::Str, &types::Array<Function<'a>>)> {
        self.0.scopes().rev().flat_map(|map| {
            map.iter().filter_map(|(key, val)| {
                if let types_rs::Value::Array(val) = val {
                    Some((key, val))
                } else {
                    None
                }
            })
        })
    }

    pub fn new_scope(&mut self, namespace: bool) { self.0.new_scope(namespace) }

    pub fn pop_scope(&mut self) { self.0.pop_scope() }

    pub fn pop_scopes<'b>(
        &'b mut self,
        index: usize,
    ) -> impl Iterator<Item = Scope<types::Str, Value<Function<'a>>>> + 'b {
        self.0.pop_scopes(index)
    }

    pub fn append_scopes(&mut self, scopes: Vec<Scope<types::Str, Value<Function<'a>>>>) {
        self.0.append_scopes(scopes)
    }

    pub fn index_scope_for_var(&self, name: &str) -> Option<usize> {
        self.0.index_scope_for_var(name)
    }

    pub fn set<T: Into<Value<Function<'a>>>>(&mut self, name: &str, value: T) {
        let value = value.into();
        if let Some(val) = self.0.get_mut(name) {
            std::mem::replace(val, value);
        } else {
            self.0.set(name, value);
        }
    }

    /// Obtains the value for the **MWD** variable.
    ///
    /// Further minimizes the directory path in the same manner that Fish does by default.
    /// That is, if more than two parents are visible in the path, all parent directories
    /// of the current directory will be reduced to a single character.
    fn get_minimal_directory(&self) -> types::Str {
        let swd = self.get_simplified_directory();

        {
            // Temporarily borrow the `swd` variable while we attempt to assemble a minimal
            // variant of the directory path. If that is not possible, we will cancel the
            // borrow and return `swd` itself as the minified path.
            let elements = swd.split('/').filter(|s| !s.is_empty()).collect::<Vec<&str>>();
            if elements.len() > 2 {
                let mut output = types::Str::new();
                for element in &elements[..elements.len() - 1] {
                    let mut segmenter = UnicodeSegmentation::graphemes(*element, true);
                    let grapheme = segmenter.next().unwrap();
                    output.push_str(grapheme);
                    if grapheme == "." {
                        output.push_str(segmenter.next().unwrap());
                    }
                    output.push('/');
                }
                output.push_str(&elements[elements.len() - 1]);
                return output;
            }
        }

        swd
    }

    /// Obtains the value for the **SWD** variable.
    ///
    /// Useful for getting smaller prompts, this will produce a simplified variant of the
    /// working directory which the leading `HOME` prefix replaced with a tilde character.
    fn get_simplified_directory(&self) -> types::Str {
        let home = self.get_str("HOME").unwrap_or_else(|_| "?".into());
        env::var("PWD").unwrap().replace(&*home, "~").into()
    }

    pub fn is_valid_variable_name(name: &str) -> bool {
        name.chars().all(Variables::is_valid_variable_character)
    }

    pub fn is_valid_variable_character(c: char) -> bool {
        c.is_alphanumeric() || c == '_' || c == '?' || c == '.' || c == '-' || c == '+'
    }

    pub fn remove_variable(&mut self, name: &str) -> Option<Value<Function<'a>>> {
        if name.starts_with("super::") || name.starts_with("global::") {
            // Cannot mutate outer namespace
            return None;
        }
        self.0.remove_variable(name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Value<Function<'a>>> {
        if name.starts_with("super::") || name.starts_with("global::") {
            // Cannot mutate outer namespace
            return None;
        }
        self.0.get_mut(name)
    }

    pub fn get_str(&self, name: &str) -> Result<types::Str, ExpansionError<IonError>> {
        match name {
            "MWD" => return Ok(self.get_minimal_directory()),
            "SWD" => return Ok(self.get_simplified_directory()),
            _ => (),
        }
        // If the parsed name contains the '::' pattern, then a namespace was
        // designated. Find it.
        match name.find("::").map(|pos| (&name[..pos], &name[pos + 2..])) {
            Some(("c", variable)) | Some(("color", variable)) => {
                Ok(Colors::collect(variable)?.to_string().into())
            }
            Some(("x", variable)) | Some(("hex", variable)) => {
                let c = u8::from_str_radix(variable, 16)
                    .map_err(|cause| ExpansionError::InvalidHex(variable.into(), cause))?;
                Ok((c as char).to_string().into())
            }
            Some(("env", variable)) => env::var(variable)
                .map(Into::into)
                .map_err(|_| ExpansionError::UnknownEnv(variable.into())),
            Some(("super", _)) | Some(("global", _)) | None => {
                // Otherwise, it's just a simple variable name.
                match self.get_ref(name) {
                    Some(Value::Str(val)) => Ok(val.clone()),
                    _ => env::var(name).map(|s| s.into()).map_err(|_| ExpansionError::VarNotFound),
                }
            }
            Some((..)) => Err(ExpansionError::UnsupportedNamespace(name.into())),
        }
    }

    pub fn get_ref(&self, mut name: &str) -> Option<&Value<Function<'a>>> {
        const GLOBAL_NS: &str = "global::";
        const SUPER_NS: &str = "super::";

        let namespace = if name.starts_with(GLOBAL_NS) {
            name = &name[GLOBAL_NS.len()..];
            // Go up as many namespaces as possible
            Namespace::Global
        } else if name.starts_with(SUPER_NS) {
            let mut up = 0;
            while name.starts_with(SUPER_NS) {
                name = &name[SUPER_NS.len()..];
                up += 1;
            }

            Namespace::Specific(up)
        } else {
            Namespace::Any
        };
        self.0.get_ref(name, namespace)
    }
}

impl<'a> Default for Variables<'a> {
    fn default() -> Self {
        let mut map: Scopes<types::Str, Value<Function<'a>>> = Scopes::with_capacity(64);
        map.set("HISTORY_SIZE", "1000");
        map.set("HISTFILE_SIZE", "100000");
        map.set(
            "PROMPT",
            "${x::1B}]0;${USER}: \
             ${PWD}${x::07}${c::0x55,bold}${USER}${c::default}:${c::0x4B}${SWD}${c::default}# \
             ${c::reset}",
        );

        // Set the PID, UID, and EUID variables.
        map.set("PID", Value::Str(getpid().ok().map_or("?".into(), |id| id.to_string().into())));
        map.set("UID", Value::Str(getuid().ok().map_or("?".into(), |id| id.to_string().into())));
        map.set("EUID", Value::Str(geteuid().ok().map_or("?".into(), |id| id.to_string().into())));

        // Initialize the HISTFILE variable
        if let Ok(base_dirs) = BaseDirectories::with_prefix("ion") {
            if let Ok(path) = base_dirs.place_data_file("history") {
                map.set("HISTFILE", path.to_str().unwrap_or("?"));
                map.set("HISTFILE_ENABLED", "1");
            }
        }

        // History Timestamps enabled variable, disabled by default
        map.set("HISTORY_TIMESTAMP", "0");

        map.set("HISTORY_IGNORE", array!["no_such_command", "whitespace", "duplicates"]);

        map.set("CDPATH", Array::new());

        // Initialize the HOME variable
        sys_env::home_dir().map_or_else(
            || env::set_var("HOME", "?"),
            |path| env::set_var("HOME", path.to_str().unwrap_or("?")),
        );

        // Initialize the HOST variable
        env::set_var("HOST", &self_sys::get_host_name().unwrap_or_else(|| "?".to_owned()));

        Variables(map)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        expansion::{Expander, Result, Select},
        shell::IonError,
    };
    use serial_test_derive::serial;

    pub struct VariableExpander<'a>(pub Variables<'a>);

    impl<'a> Expander for VariableExpander<'a> {
        type Error = IonError;

        fn string(&self, var: &str) -> Result<types::Str, IonError> { self.0.get_str(var) }

        fn array(&self, _variable: &str, _selection: &Select) -> Result<types::Args, Self::Error> {
            Err(ExpansionError::VarNotFound)
        }

        fn command(&self, cmd: &str) -> Result<types::Str, Self::Error> { Ok(cmd.into()) }

        fn tilde(&self, input: &str) -> Result<types::Str, Self::Error> { Ok(input.into()) }

        fn map_keys(&self, _name: &str, _select: &Select) -> Result<types::Args, Self::Error> {
            Err(ExpansionError::VarNotFound)
        }

        fn map_values(&self, _name: &str, _select: &Select) -> Result<types::Args, Self::Error> {
            Err(ExpansionError::VarNotFound)
        }
    }

    #[test]
    fn undefined_variable_errors() {
        let variables = Variables::default();
        assert!(VariableExpander(variables).expand_string("$FOO").is_err());
    }

    #[test]
    fn set_var_and_expand_a_variable() {
        let mut variables = Variables::default();
        variables.set("FOO", "BAR");
        let expanded = VariableExpander(variables).expand_string("$FOO").unwrap().join("");
        assert_eq!("BAR", &expanded);
    }

    #[test]
    #[serial]
    fn minimal_directory_var_should_compact_path() {
        let variables = Variables::default();
        env::set_var("PWD", "/var/log/nix");
        assert_eq!(
            types::Str::from("v/l/nix"),
            variables.get_str("MWD").expect("no value returned"),
        );
    }

    #[test]
    #[serial]
    fn minimal_directory_var_shouldnt_compact_path() {
        let variables = Variables::default();
        env::set_var("PWD", "/var/log");
        assert_eq!(
            types::Str::from("/var/log"),
            variables.get_str("MWD").expect("no value returned"),
        );
    }
}
