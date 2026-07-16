pub mod session;

/// Encode a project path for use as a directory name.
/// /home/user/project → -home-user-project
pub fn encode_project_path(path: &str) -> String {
    path.replace('/', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_project_path() {
        assert_eq!(
            encode_project_path("/home/user/projects/myapp"),
            "-home-user-projects-myapp"
        );
    }
}
