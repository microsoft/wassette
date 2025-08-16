// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

// Test for environment variable parsing functionality
use std::collections::HashMap;

#[cfg(test)]
mod env_var_tests {
    use super::*;
    
    // Test the parse_env_var function
    #[test]
    fn test_parse_env_var_valid() {
        let result = crate::parse_env_var("KEY=value");
        assert!(result.is_ok());
        let (key, value) = result.unwrap();
        assert_eq!(key, "KEY");
        assert_eq!(value, "value");
    }
    
    #[test]
    fn test_parse_env_var_with_equals_in_value() {
        let result = crate::parse_env_var("DATABASE_URL=postgres://user:pass@host:5432/db?option=value");
        assert!(result.is_ok());
        let (key, value) = result.unwrap();
        assert_eq!(key, "DATABASE_URL");
        assert_eq!(value, "postgres://user:pass@host:5432/db?option=value");
    }
    
    #[test]
    fn test_parse_env_var_empty_value() {
        let result = crate::parse_env_var("EMPTY=");
        assert!(result.is_ok());
        let (key, value) = result.unwrap();
        assert_eq!(key, "EMPTY");
        assert_eq!(value, "");
    }
    
    #[test]
    fn test_parse_env_var_invalid_format() {
        let result = crate::parse_env_var("INVALID_NO_EQUALS");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("KEY=VALUE format"));
    }
    
    #[test]
    fn test_parse_env_var_empty_key() {
        let result = crate::parse_env_var("=value");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("key cannot be empty"));
    }
    
    // Test the load_env_file function
    #[test]
    fn test_load_env_file_basic() {
        use std::io::Write;
        use tempfile::NamedTempFile;
        
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "KEY1=value1").unwrap();
        writeln!(temp_file, "KEY2=value2").unwrap();
        writeln!(temp_file, "# This is a comment").unwrap();
        writeln!(temp_file, "").unwrap(); // Empty line
        writeln!(temp_file, "KEY3=value3").unwrap();
        
        let result = crate::load_env_file(&temp_file.path().to_path_buf());
        assert!(result.is_ok());
        
        let env_vars = result.unwrap();
        assert_eq!(env_vars.len(), 3);
        assert_eq!(env_vars.get("KEY1"), Some(&"value1".to_string()));
        assert_eq!(env_vars.get("KEY2"), Some(&"value2".to_string()));
        assert_eq!(env_vars.get("KEY3"), Some(&"value3".to_string()));
    }
    
    #[test]
    fn test_load_env_file_quoted_values() {
        use std::io::Write;
        use tempfile::NamedTempFile;
        
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, r#"QUOTED_DOUBLE="double quoted value""#).unwrap();
        writeln!(temp_file, r#"QUOTED_SINGLE='single quoted value'"#).unwrap();
        writeln!(temp_file, r#"UNQUOTED=unquoted value"#).unwrap();
        
        let result = crate::load_env_file(&temp_file.path().to_path_buf());
        assert!(result.is_ok());
        
        let env_vars = result.unwrap();
        assert_eq!(env_vars.get("QUOTED_DOUBLE"), Some(&"double quoted value".to_string()));
        assert_eq!(env_vars.get("QUOTED_SINGLE"), Some(&"single quoted value".to_string()));
        assert_eq!(env_vars.get("UNQUOTED"), Some(&"unquoted value".to_string()));
    }
    
    #[test]
    fn test_load_env_file_invalid_format() {
        use std::io::Write;
        use tempfile::NamedTempFile;
        
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "VALID=value").unwrap();
        writeln!(temp_file, "INVALID_LINE_NO_EQUALS").unwrap();
        
        let result = crate::load_env_file(&temp_file.path().to_path_buf());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid environment variable format"));
    }
}
