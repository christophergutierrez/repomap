const MAX_RETRIES: u32 = 3;

/// A user struct
struct User {
    id: u32,
    name: String,
}

trait Authenticatable {
    fn authenticate(&self, token: &str) -> bool;
}

impl Authenticatable for User {
    fn authenticate(&self, token: &str) -> bool {
        !token.is_empty()
    }
}

impl User {
    /// Create a new user
    fn new(id: u32, name: String) -> Self {
        User { id, name }
    }
}

impl std::fmt::Display for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// Authenticate a token
fn authenticate(token: &str) -> bool {
    !token.is_empty()
}
