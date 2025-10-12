/// Path matching module with wildcard support
///
/// Supports wildcard patterns where * can appear anywhere in the pattern
/// Example patterns: "/api/*", "/*/users", "/api/*/data"
/// Also supports method prefixes: "POST /api/*", "GET *", "PUT /hello"

/// Parse a pattern into optional method and path parts
/// Returns (method, path_pattern)
/// Examples:
///   "POST /api/*" -> (Some("POST"), "/api/*")
///   "/api/*" -> (None, "/api/*")
///   "GET *" -> (Some("GET"), "*")
fn parse_pattern(pattern: &str) -> (Option<&str>, &str) {
    let pattern = pattern.trim();
    
    // Check if pattern starts with an HTTP method
    let methods = ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "CONNECT", "TRACE"];
    
    for method in &methods {
        if pattern.starts_with(method) {
            let rest = &pattern[method.len()..];
            // Must be followed by whitespace
            if rest.starts_with(' ') || rest.starts_with('\t') {
                let path_pattern = rest.trim_start();
                return (Some(method), path_pattern);
            }
        }
    }
    
    (None, pattern)
}

/// Check if a path matches a wildcard pattern
/// * can appear anywhere and matches any sequence of characters
/// If method is provided, pattern can optionally specify a method prefix like "POST /api/*"
pub fn matches_pattern(path: &str, pattern: &str) -> bool {
    matches_pattern_with_method(None, path, pattern)
}

/// Check if a request (method + path) matches a pattern
/// Pattern can be just a path or "METHOD /path"
/// Examples:
///   matches_pattern_with_method(Some("POST"), "/api/users", "POST /api/*") -> true
///   matches_pattern_with_method(Some("GET"), "/api/users", "POST /api/*") -> false
///   matches_pattern_with_method(Some("GET"), "/api/users", "/api/*") -> true (no method constraint)
pub fn matches_pattern_with_method(method: Option<&str>, path: &str, pattern: &str) -> bool {
    let (pattern_method, path_pattern) = parse_pattern(pattern);
    
    // If pattern specifies a method, it must match
    if let Some(required_method) = pattern_method {
        if let Some(actual_method) = method {
            if required_method != actual_method {
                return false;
            }
        } else {
            // Pattern requires a method but none was provided
            return false;
        }
    }
    
    // Now match the path part using the existing logic
    matches_path_pattern(path, path_pattern)
}

/// Internal function to match just the path against a pattern
fn matches_path_pattern(path: &str, pattern: &str) -> bool {
    // Split pattern by * to get segments
    let segments: Vec<&str> = pattern.split('*').collect();
    
    if segments.len() == 1 {
        // No wildcards, exact match
        return path == pattern;
    }
    
    let mut current_pos = 0;
    
    for (i, segment) in segments.iter().enumerate() {
        if i == 0 {
            // First segment must match at the start
            if !segment.is_empty() && !path.starts_with(segment) {
                return false;
            }
            current_pos = segment.len();
        } else if i == segments.len() - 1 {
            // Last segment must match at the end
            if !segment.is_empty() && !path.ends_with(segment) {
                return false;
            }
            // Also ensure that the last segment appears after current_pos
            if !segment.is_empty() {
                if let Some(pos) = path[current_pos..].find(segment) {
                    if current_pos + pos + segment.len() != path.len() {
                        return false;
                    }
                } else {
                    return false;
                }
            }
        } else {
            // Middle segments must appear in order
            if let Some(pos) = path[current_pos..].find(segment) {
                current_pos += pos + segment.len();
            } else {
                return false;
            }
        }
    }
    
    true
}

/// Check if a request should be cached based on include and exclude patterns
/// - If include_paths is empty, all paths are included
/// - If exclude_paths is empty, no paths are excluded
/// - exclude_paths overrides include_paths
/// - Patterns can include method prefixes: "POST /api/*", "GET *", etc.
pub fn should_cache_path(
    method: &str,
    path: &str,
    include_paths: &[String],
    exclude_paths: &[String],
) -> bool {
    // Check exclude patterns first (they override includes)
    if !exclude_paths.is_empty() {
        for pattern in exclude_paths {
            if matches_pattern_with_method(Some(method), path, pattern) {
                return false;
            }
        }
    }
    
    // If include_paths is empty, include everything (that wasn't excluded)
    if include_paths.is_empty() {
        return true;
    }
    
    // Check if path matches any include pattern
    for pattern in include_paths {
        if matches_pattern_with_method(Some(method), path, pattern) {
            return true;
        }
    }
    
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        assert!(matches_pattern("/api/users", "/api/users"));
        assert!(!matches_pattern("/api/users", "/api/posts"));
    }

    #[test]
    fn test_wildcard_at_end() {
        assert!(matches_pattern("/api/users", "/api/*"));
        assert!(matches_pattern("/api/users/123", "/api/*"));
        assert!(!matches_pattern("/apiv2/users", "/api/*"));
    }

    #[test]
    fn test_wildcard_at_start() {
        assert!(matches_pattern("/api/users", "*/users"));
        assert!(matches_pattern("/v1/api/users", "*/users"));
        assert!(!matches_pattern("/api/posts", "*/users"));
    }

    #[test]
    fn test_wildcard_in_middle() {
        assert!(matches_pattern("/api/v1/users", "/api/*/users"));
        assert!(matches_pattern("/api/v2/users", "/api/*/users"));
        assert!(!matches_pattern("/api/v1/posts", "/api/*/users"));
    }

    #[test]
    fn test_multiple_wildcards() {
        assert!(matches_pattern("/api/v1/users/123", "/api/*/users/*"));
        assert!(matches_pattern("/api/v2/users/456", "/api/*/users/*"));
        assert!(!matches_pattern("/api/v1/posts/123", "/api/*/users/*"));
    }

    #[test]
    fn test_wildcard_only() {
        assert!(matches_pattern("/anything", "*"));
        assert!(matches_pattern("/api/users/123", "*"));
    }

    #[test]
    fn test_should_cache_path_empty_filters() {
        // Empty include and exclude should cache everything
        assert!(should_cache_path("GET", "/api/users", &[], &[]));
        assert!(should_cache_path("POST", "/anything", &[], &[]));
    }

    #[test]
    fn test_should_cache_path_include_only() {
        let include = vec!["/api/*".to_string(), "/public/*".to_string()];
        let exclude = vec![];
        
        assert!(should_cache_path("GET", "/api/users", &include, &exclude));
        assert!(should_cache_path("GET", "/public/index.html", &include, &exclude));
        assert!(!should_cache_path("GET", "/private/data", &include, &exclude));
    }

    #[test]
    fn test_should_cache_path_exclude_only() {
        let include = vec![];
        let exclude = vec!["/admin/*".to_string(), "/private/*".to_string()];
        
        assert!(should_cache_path("GET", "/api/users", &include, &exclude));
        assert!(!should_cache_path("GET", "/admin/dashboard", &include, &exclude));
        assert!(!should_cache_path("GET", "/private/data", &include, &exclude));
    }

    #[test]
    fn test_should_cache_path_exclude_overrides_include() {
        let include = vec!["/api/*".to_string()];
        let exclude = vec!["/api/admin/*".to_string()];
        
        assert!(should_cache_path("GET", "/api/users", &include, &exclude));
        assert!(!should_cache_path("GET", "/api/admin/users", &include, &exclude));
    }

    #[test]
    fn test_method_pattern_matching() {
        // Test exact method match
        assert!(matches_pattern_with_method(Some("POST"), "/api/users", "POST /api/users"));
        assert!(!matches_pattern_with_method(Some("GET"), "/api/users", "POST /api/users"));
        
        // Test method with wildcard
        assert!(matches_pattern_with_method(Some("POST"), "/api/users", "POST /api/*"));
        assert!(matches_pattern_with_method(Some("POST"), "/api/posts", "POST /api/*"));
        assert!(!matches_pattern_with_method(Some("POST"), "/not-api/posts", "POST /api/*"));
        assert!(!matches_pattern_with_method(Some("GET"), "/api/users", "POST /api/*"));
        
        // Test wildcard method matching (pattern without method should match any)
        assert!(matches_pattern_with_method(Some("GET"), "/api/users", "/api/*"));
        assert!(matches_pattern_with_method(Some("POST"), "/api/users", "/api/*"));
        
        // Test "POST *" pattern
        assert!(matches_pattern_with_method(Some("POST"), "/anything", "POST *"));
        assert!(matches_pattern_with_method(Some("POST"), "/api/users/123", "POST *"));
        assert!(!matches_pattern_with_method(Some("GET"), "/anything", "POST *"));
    }

    #[test]
    fn test_should_cache_with_method_filters() {
        let include = vec!["/api/*".to_string()];
        let exclude = vec!["POST /api/*".to_string(), "PUT /api/*".to_string()];
        
        // GET should be cached
        assert!(should_cache_path("GET", "/api/users", &include, &exclude));
        // POST should not be cached (excluded)
        assert!(!should_cache_path("POST", "/api/users", &include, &exclude));
        // PUT should not be cached (excluded)
        assert!(!should_cache_path("PUT", "/api/users", &include, &exclude));
        // DELETE should be cached (not excluded)
        assert!(should_cache_path("DELETE", "/api/users", &include, &exclude));
    }

    #[test]
    fn test_exclude_all_posts() {
        let include = vec![];
        let exclude = vec!["POST *".to_string()];
        
        // All POST requests should be excluded
        assert!(!should_cache_path("POST", "/api/users", &include, &exclude));
        assert!(!should_cache_path("POST", "/anything", &include, &exclude));
        
        // Other methods should be cached
        assert!(should_cache_path("GET", "/api/users", &include, &exclude));
        assert!(should_cache_path("PUT", "/api/users", &include, &exclude));
    }

    #[test]
    fn test_include_only_get_requests() {
        let include = vec!["GET *".to_string()];
        let exclude = vec![];
        
        // Only GET requests should be included
        assert!(should_cache_path("GET", "/api/users", &include, &exclude));
        assert!(should_cache_path("GET", "/anything", &include, &exclude));
        
        // Other methods should not be cached
        assert!(!should_cache_path("POST", "/api/users", &include, &exclude));
        assert!(!should_cache_path("PUT", "/api/users", &include, &exclude));
    }
}
