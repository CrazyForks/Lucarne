use std::io::{self, BufRead, IsTerminal, Write};

pub(crate) trait OnboardingTerminal {
    fn is_interactive(&self) -> bool;

    fn println(&mut self, message: &str) -> io::Result<()>;

    fn prompt(&mut self, label: &str, default: Option<&str>) -> io::Result<String>;

    fn confirm(&mut self, label: &str, default: bool) -> io::Result<bool> {
        loop {
            let default_label = if default { "Y/n" } else { "y/N" };
            let answer = self.prompt(&format!("{label} [{default_label}]"), None)?;
            let answer = answer.trim().to_ascii_lowercase();
            match answer.as_str() {
                "" => return Ok(default),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => self.println("Please answer y or n.")?,
            }
        }
    }
}

pub(crate) struct StdioTerminal {
    stdin: io::Stdin,
    stdout: io::Stdout,
}

impl StdioTerminal {
    pub(crate) fn new() -> Self {
        Self {
            stdin: io::stdin(),
            stdout: io::stdout(),
        }
    }
}

impl Default for StdioTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl OnboardingTerminal for StdioTerminal {
    fn is_interactive(&self) -> bool {
        self.stdin.is_terminal() && self.stdout.is_terminal()
    }

    fn println(&mut self, message: &str) -> io::Result<()> {
        writeln!(self.stdout, "{message}")
    }

    fn prompt(&mut self, label: &str, default: Option<&str>) -> io::Result<String> {
        match default {
            Some(default) => write!(self.stdout, "{label} [{default}]: ")?,
            None => write!(self.stdout, "{label}: ")?,
        }
        self.stdout.flush()?;

        let mut answer = String::new();
        self.stdin.lock().read_line(&mut answer)?;
        let answer = answer.trim_end_matches(['\r', '\n']).to_string();
        if answer.is_empty() {
            Ok(default.unwrap_or_default().to_string())
        } else {
            Ok(answer)
        }
    }
}

#[cfg(test)]
pub(crate) struct ScriptedTerminal {
    answers: std::collections::VecDeque<String>,
    output: String,
}

#[cfg(test)]
impl ScriptedTerminal {
    pub(crate) fn new<I, S>(answers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            answers: answers.into_iter().map(Into::into).collect(),
            output: String::new(),
        }
    }

    pub(crate) fn output(&self) -> &str {
        &self.output
    }
}

#[cfg(test)]
impl OnboardingTerminal for ScriptedTerminal {
    fn is_interactive(&self) -> bool {
        true
    }

    fn println(&mut self, message: &str) -> io::Result<()> {
        self.output.push_str(message);
        self.output.push('\n');
        Ok(())
    }

    fn prompt(&mut self, label: &str, default: Option<&str>) -> io::Result<String> {
        match default {
            Some(default) => self.output.push_str(&format!("{label} [{default}]:\n")),
            None => self.output.push_str(&format!("{label}:\n")),
        }

        let answer = self.answers.pop_front().ok_or_else(|| {
            io::Error::new(io::ErrorKind::UnexpectedEof, "missing scripted answer")
        })?;
        if answer.is_empty() {
            Ok(default.unwrap_or_default().to_string())
        } else {
            Ok(answer)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_terminal_reads_answers_and_records_output() {
        let mut terminal = ScriptedTerminal::new(["y", "codex, pi"]);
        assert!(terminal
            .confirm("Enable Telegram?", false)
            .expect("confirm"));
        assert_eq!(
            terminal.prompt("Agents", Some("all")).expect("prompt"),
            "codex, pi"
        );
        assert!(terminal.output().contains("Enable Telegram?"));
        assert!(terminal.output().contains("Agents"));
    }

    #[test]
    fn scripted_terminal_uses_default_for_empty_answer() {
        let mut terminal = ScriptedTerminal::new([""]);
        assert_eq!(
            terminal.prompt("Token", Some("existing")).expect("prompt"),
            "existing"
        );
    }

    #[test]
    fn confirm_uses_default_true_on_empty_answer() {
        let mut terminal = ScriptedTerminal::new([""]);

        assert!(terminal.confirm("Enable Telegram?", true).expect("confirm"));
        assert!(!terminal.output().contains("Please answer y or n."));
    }

    #[test]
    fn confirm_uses_default_false_on_empty_answer() {
        let mut terminal = ScriptedTerminal::new([""]);

        assert!(!terminal
            .confirm("Enable Telegram?", false)
            .expect("confirm"));
        assert!(!terminal.output().contains("Please answer y or n."));
    }
}
