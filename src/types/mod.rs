// ProbeDB 类型系统 — VECTOR 是一等公民

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum DataType {
    Integer,
    Float,
    Text,
    /// 布尔类型：TRUE / FALSE
    Boolean,
    /// 日期类型：ISO 8601 `YYYY-MM-DD`
    /// 内部按规范化字符串存储 —— 零填充保证**字典序 == 时间序**，无需日期运算库
    Date,
    /// 时间类型：ISO 8601 `HH:MM:SS`
    Time,
    /// 向量类型，dimension 表示向量维度
    Vector(usize),
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataType::Integer => write!(f, "INTEGER"),
            DataType::Float => write!(f, "FLOAT"),
            DataType::Text => write!(f, "TEXT"),
            DataType::Boolean => write!(f, "BOOLEAN"),
            DataType::Date => write!(f, "DATE"),
            DataType::Time => write!(f, "TIME"),
            DataType::Vector(dim) => write!(f, "VECTOR({})", dim),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Integer(i64),
    Float(f64),
    Text(String),
    Boolean(bool),
    /// 规范化日期 `YYYY-MM-DD`（写入时校验，非法日期直接报错）
    Date(String),
    /// 规范化时间 `HH:MM:SS`
    Time(String),
    Vector(Vec<f64>),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Integer(v) => write!(f, "{}", v),
            Value::Float(v) => write!(f, "{}", v),
            Value::Text(v) => write!(f, "{}", v),
            Value::Boolean(v) => write!(f, "{}", if *v { "true" } else { "false" }),
            Value::Date(v) => write!(f, "{}", v),
            Value::Time(v) => write!(f, "{}", v),
            Value::Vector(v) => {
                let dims: Vec<String> = v.iter().map(|x| x.to_string()).collect();
                write!(f, "[{}]", dims.join(","))
            }
        }
    }
}

// ===== 时间/日期字面量校验与规范化（零外部依赖） =====

/// 闰年判定（格里高利历规则：4 年一闰，100 年不闰，400 年再闰）
pub fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// 某年某月的天数（月份非法返回 0）
pub fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap_year(year) { 29 } else { 28 },
        _ => 0,
    }
}

/// 校验并规范化日期字面量 → `YYYY-MM-DD`
///
/// 只接受严格的 `YYYY-MM-DD`（月/日必须零填充），非法日期（如 `2026-02-30`、
/// `2025-02-29`）明确报错——这是 DATE 类型区别于 TEXT 的语义保证。
pub fn normalize_date(input: &str) -> Result<String, String> {
    let s = input.trim().trim_matches('\'');
    let parts: Vec<&str> = s.split('-').collect();
    let shape_ok = parts.len() == 3
        && parts[0].len() == 4
        && parts[1].len() == 2
        && parts[2].len() == 2
        && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()));
    if !shape_ok {
        return Err(format!("日期格式应为 YYYY-MM-DD: {}", input));
    }
    let year: i64 = parts[0].parse().map_err(|_| format!("日期年份解析失败: {}", input))?;
    let month: i64 = parts[1].parse().map_err(|_| format!("日期月份解析失败: {}", input))?;
    let day: i64 = parts[2].parse().map_err(|_| format!("日期日解析失败: {}", input))?;
    if year < 1 || year > 9999 {
        return Err(format!("日期年份超出范围(1-9999): {}", input));
    }
    if month < 1 || month > 12 {
        return Err(format!("日期月份超出范围(1-12): {}", input));
    }
    let max_day = days_in_month(year, month);
    if day < 1 || day > max_day {
        return Err(format!("日期 {} 的日超出范围(1-{}): {}", s, max_day, input));
    }
    Ok(format!("{:04}-{:02}-{:02}", year, month, day))
}

/// 校验并规范化时间字面量 → `HH:MM:SS`
///
/// 接受 `HH:MM:SS` 或 `HH:MM`（秒补 00）；非法时间（如 `25:00:00`）明确报错。
pub fn normalize_time(input: &str) -> Result<String, String> {
    let s = input.trim().trim_matches('\'');
    let parts: Vec<&str> = s.split(':').collect();
    if (parts.len() != 2 && parts.len() != 3)
        || !parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_digit()))
    {
        return Err(format!("时间格式应为 HH:MM:SS 或 HH:MM: {}", input));
    }
    let hour: i64 = parts[0].parse().map_err(|_| format!("时间小时解析失败: {}", input))?;
    let minute: i64 = parts[1].parse().map_err(|_| format!("时间分钟解析失败: {}", input))?;
    let second: i64 = if parts.len() == 3 {
        parts[2].parse().map_err(|_| format!("时间秒解析失败: {}", input))?
    } else {
        0
    };
    if hour > 23 {
        return Err(format!("时间小时超出范围(0-23): {}", input));
    }
    if minute > 59 {
        return Err(format!("时间分钟超出范围(0-59): {}", input));
    }
    if second > 59 {
        return Err(format!("时间秒超出范围(0-59): {}", input));
    }
    Ok(format!("{:02}:{:02}:{:02}", hour, minute, second))
}

/// 向量相似度类型
#[derive(Debug, Clone, PartialEq)]
pub enum SimilarityMetric {
    Cosine,
    Euclidean,
    DotProduct,
}

/// 向量相似度计算结果
#[derive(Debug, Clone)]
pub struct SimilarityResult {
    pub value: f64,
    pub row_id: u64,
}

/// 计算两个向量的余弦相似度
pub fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// 计算欧几里得距离（转换为相似度：1/(1+distance)）
pub fn euclidean_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dist: f64 = a.iter().zip(b.iter()).map(|(x, y)| (x - y).powi(2)).sum();
    let dist = dist.sqrt();
    1.0 / (1.0 + dist)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let v = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_vector_display() {
        let v = Value::Vector(vec![1.0, 2.0, 3.0]);
        assert_eq!(v.to_string(), "[1,2,3]");
    }

    #[test]
    fn test_euclidean_similarity_identical() {
        let v = vec![1.0, 2.0, 3.0];
        let sim = euclidean_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    // ===== BOOLEAN / DATE / TIME 类型系统 =====

    #[test]
    fn test_boolean_and_temporal_display() {
        assert_eq!(Value::Boolean(true).to_string(), "true");
        assert_eq!(Value::Boolean(false).to_string(), "false");
        assert_eq!(Value::Date("2026-09-16".to_string()).to_string(), "2026-09-16");
        assert_eq!(Value::Time("14:00:00".to_string()).to_string(), "14:00:00");
        assert_eq!(DataType::Boolean.to_string(), "BOOLEAN");
        assert_eq!(DataType::Date.to_string(), "DATE");
        assert_eq!(DataType::Time.to_string(), "TIME");
    }

    #[test]
    fn test_normalize_date_valid() {
        assert_eq!(normalize_date("2026-09-16").unwrap(), "2026-09-16");
        assert_eq!(normalize_date("'2026-09-16'").unwrap(), "2026-09-16"); // 带引号
        assert_eq!(normalize_date(" 2026-01-01 ").unwrap(), "2026-01-01"); // 带空格
        assert_eq!(normalize_date("0001-01-01").unwrap(), "0001-01-01"); // 下界
        assert_eq!(normalize_date("9999-12-31").unwrap(), "9999-12-31"); // 上界
    }

    #[test]
    fn test_normalize_date_rejects_invalid() {
        // 形态错误
        assert!(normalize_date("").is_err());
        assert!(normalize_date("2026/09/16").is_err());
        assert!(normalize_date("2026-9-16").is_err(), "月份必须零填充");
        assert!(normalize_date("26-09-16").is_err(), "年份必须 4 位");
        assert!(normalize_date("2026-09").is_err());
        assert!(normalize_date("20260916").is_err());
        assert!(normalize_date("abcd-ef-gh").is_err());
        // 范围错误
        assert!(normalize_date("2026-00-01").is_err(), "月份下界");
        assert!(normalize_date("2026-13-01").is_err(), "月份上界");
        assert!(normalize_date("2026-01-00").is_err(), "日下界");
        assert!(normalize_date("2026-01-32").is_err(), "1月只有31天");
        assert!(normalize_date("2026-04-31").is_err(), "4月只有30天");
        assert!(normalize_date("0000-01-01").is_err(), "年份下界");
    }

    #[test]
    fn test_normalize_date_leap_year() {
        assert!(is_leap_year(2024));
        assert!(is_leap_year(2000), "400年再闰");
        assert!(!is_leap_year(1900), "100年不闰");
        assert!(!is_leap_year(2025));
        assert!(normalize_date("2024-02-29").is_ok(), "闰年2月有29天");
        assert!(normalize_date("2025-02-29").is_err(), "平年2月只有28天");
        assert!(normalize_date("2000-02-29").is_ok());
        assert!(normalize_date("1900-02-29").is_err());
        assert!(normalize_date("2026-02-29").is_err());
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2025, 2), 28);
        assert_eq!(days_in_month(2026, 7), 31);
        assert_eq!(days_in_month(2026, 13), 0);
    }

    #[test]
    fn test_normalize_time_valid() {
        assert_eq!(normalize_time("14:00:00").unwrap(), "14:00:00");
        assert_eq!(normalize_time("'14:00:00'").unwrap(), "14:00:00");
        assert_eq!(normalize_time("14:00").unwrap(), "14:00:00", "HH:MM 秒补零");
        assert_eq!(normalize_time("00:00:00").unwrap(), "00:00:00");
        assert_eq!(normalize_time("23:59:59").unwrap(), "23:59:59");
    }

    #[test]
    fn test_normalize_time_rejects_invalid() {
        assert!(normalize_time("").is_err());
        assert!(normalize_time("24:00:00").is_err(), "小时上界");
        assert!(normalize_time("14:60:00").is_err(), "分钟上界");
        assert!(normalize_time("14:00:60").is_err(), "秒上界");
        assert!(normalize_time("4:00:00").is_err(), "必须零填充");
        assert!(normalize_time("14:0").is_err());
        assert!(normalize_time("14-00-00").is_err());
        assert!(normalize_time("abc").is_err());
    }

    #[test]
    fn test_iso_date_string_order_equals_chronological_order() {
        // DATE 存规范化字符串的核心保证：字典序 == 时间序（跨月/跨年/跨闰日）
        let mut dates = vec!["2026-12-31", "2026-01-05", "2026-02-28", "2025-12-31", "2026-09-16"];
        dates.sort();
        assert_eq!(
            dates,
            vec!["2025-12-31", "2026-01-05", "2026-02-28", "2026-09-16", "2026-12-31"]
        );
    }
}