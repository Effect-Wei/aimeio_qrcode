/* ========================================================================= */
/* ========================= 纯 Rust INI 解析器 ========================== */
/* ========================================================================= */

pub struct IniParser {
    path: std::path::PathBuf,
    data: std::collections::HashMap<String, std::collections::HashMap<String, String>>,
}

impl IniParser {
    fn read_ini_file(
        path: &std::path::Path,
    ) -> std::io::Result<std::collections::HashMap<String, std::collections::HashMap<String, String>>>
    {
        use std::io::BufRead;
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        let mut data = std::collections::HashMap::new();
        let mut current_section = "default".to_string();

        for line in reader.lines() {
            let line = line?.trim().to_string();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                current_section = line[1..line.len() - 1].to_string();
            } else if let Some((key, value)) = line.split_once('=') {
                data.entry(current_section.clone())
                    .or_insert_with(std::collections::HashMap::new)
                    .insert(key.trim().to_string(), value.trim().to_string());
            }
        }
        Ok(data)
    }

    pub fn new(path: &std::path::Path) -> std::io::Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
            data: Self::read_ini_file(path)?,
        })
    }

    pub fn reload(&mut self) -> std::io::Result<()> {
        self.data = Self::read_ini_file(&self.path)?;
        Ok(())
    }

    pub fn get_string(&self, section: &str, key: &str, default: &str) -> String {
        self.data
            .get(section)
            .and_then(|sec| sec.get(key))
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }

    pub fn get_int(&self, section: &str, key: &str, default: i32) -> i32 {
        self.get_string(section, key, "")
            .parse::<i32>()
            .unwrap_or(default)
    }
}
