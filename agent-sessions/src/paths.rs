use std::path::PathBuf;

pub(crate) fn home_dir() -> Option<PathBuf> {
    home_dir_from_env(EnvReader)
}

#[cfg(not(windows))]
fn home_dir_from_env(env: impl Env) -> Option<PathBuf> {
    env.var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(windows)]
fn home_dir_from_env(env: impl Env) -> Option<PathBuf> {
    env.var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env.var_os("USERPROFILE")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| {
            let drive = env.var_os("HOMEDRIVE").filter(|value| !value.is_empty())?;
            let path = env.var_os("HOMEPATH").filter(|value| !value.is_empty())?;
            Some(PathBuf::from(format!(
                "{}{}",
                drive.to_string_lossy(),
                path.to_string_lossy()
            )))
        })
}

trait Env: Copy {
    fn var_os(self, name: &str) -> Option<std::ffi::OsString>;
}

#[derive(Clone, Copy)]
struct EnvReader;

impl Env for EnvReader {
    fn var_os(self, name: &str) -> Option<std::ffi::OsString> {
        std::env::var_os(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use std::collections::BTreeMap;
    #[cfg(windows)]
    use std::ffi::OsString;

    #[cfg(windows)]
    #[derive(Clone, Copy)]
    struct MapEnv<'a>(&'a BTreeMap<&'a str, &'a str>);

    #[cfg(windows)]
    impl Env for MapEnv<'_> {
        fn var_os(self, name: &str) -> Option<OsString> {
            self.0.get(name).map(OsString::from)
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_home_dir_uses_home_drive_and_home_path() {
        let env = BTreeMap::from([("HOMEDRIVE", r"C:"), ("HOMEPATH", r"\Users\alice")]);
        assert_eq!(
            home_dir_from_env(MapEnv(&env)),
            Some(PathBuf::from(r"C:\Users\alice"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_home_dir_ignores_empty_home_drive_and_home_path() {
        let env = BTreeMap::from([("HOMEDRIVE", ""), ("HOMEPATH", r"\Users\alice")]);
        assert_eq!(home_dir_from_env(MapEnv(&env)), None);
    }
}
