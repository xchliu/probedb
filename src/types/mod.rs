// ProbeDB 类型系统 — VECTOR 是一等公民

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum DataType {
    Integer,
    Float,
    Text,
    /// 向量类型，dimension 表示向量维度
    Vector(usize),
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataType::Integer => write!(f, "INTEGER"),
            DataType::Float => write!(f, "FLOAT"),
            DataType::Text => write!(f, "TEXT"),
            DataType::Vector(dim) => write!(f, "VECTOR({})", dim),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Integer(i64),
    Float(f64),
    Text(String),
    Vector(Vec<f64>),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Integer(v) => write!(f, "{}", v),
            Value::Float(v) => write!(f, "{}", v),
            Value::Text(v) => write!(f, "{}", v),
            Value::Vector(v) => {
                let dims: Vec<String> = v.iter().map(|x| x.to_string()).collect();
                write!(f, "[{}]", dims.join(","))
            }
        }
    }
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
}