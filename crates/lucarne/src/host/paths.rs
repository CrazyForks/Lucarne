use std::path::PathBuf;

pub(crate) fn default_lucarned_home_dir() -> Option<PathBuf> {
    default_lucarned_home_dir_from_env(EnvReader)
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    home_dir_from_env(EnvReader)
}

#[cfg(unix)]
fn default_lucarned_home_dir_from_env(env: impl Env) -> Option<PathBuf> {
    home_dir_from_env(env).map(|home| home.join(".lucarned"))
}

#[cfg(unix)]
fn home_dir_from_env(env: impl Env) -> Option<PathBuf> {
    env.var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(windows)]
fn default_lucarned_home_dir_from_env(env: impl Env) -> Option<PathBuf> {
    env.var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|base| base.join("lucarned"))
        .or_else(|| home_dir_from_env(env).map(|home| home.join(".lucarned")))
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

pub(crate) fn default_state_db_path() -> Option<PathBuf> {
    default_lucarned_home_dir().map(|home| home.join("state.sqlite3"))
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
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    #[derive(Clone, Copy)]
    struct MapEnv<'a>(&'a BTreeMap<&'a str, &'a str>);

    impl Env for MapEnv<'_> {
        fn var_os(self, name: &str) -> Option<OsString> {
            self.0.get(name).map(OsString::from)
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_default_home_uses_home_dot_lucarned() {
        let env = BTreeMap::from([("HOME", "/home/alice")]);
        let path = default_lucarned_home_dir_from_env(MapEnv(&env)).expect("path");
        assert_eq!(path, PathBuf::from("/home/alice/.lucarned"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_default_home_prefers_local_app_data() {
        let env = BTreeMap::from([
            ("LOCALAPPDATA", r"C:\Users\alice\AppData\Local"),
            ("USERPROFILE", r"C:\Users\alice"),
        ]);
        let path = default_lucarned_home_dir_from_env(MapEnv(&env)).expect("path");
        assert_eq!(
            path,
            PathBuf::from(r"C:\Users\alice\AppData\Local\lucarned")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_default_home_falls_back_to_user_profile() {
        let env = BTreeMap::from([("USERPROFILE", r"C:\Users\alice")]);
        let path = default_lucarned_home_dir_from_env(MapEnv(&env)).expect("path");
        assert_eq!(path, PathBuf::from(r"C:\Users\alice\.lucarned"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_home_dir_uses_home_drive_and_home_path() {
        let env = BTreeMap::from([("HOMEDRIVE", r"C:"), ("HOMEPATH", r"\Users\alice")]);
        let path = home_dir_from_env(MapEnv(&env)).expect("path");
        assert_eq!(path, PathBuf::from(r"C:\Users\alice"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_home_dir_ignores_empty_home_drive_and_home_path() {
        let env = BTreeMap::from([("HOMEDRIVE", ""), ("HOMEPATH", r"\Users\alice")]);
        assert_eq!(home_dir_from_env(MapEnv(&env)), None);
    }
}
