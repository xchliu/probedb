// ProbeDB — AI内建数据库（二进制入口）
// 库代码在 src/lib.rs；本文件仅负责演示 + 冒烟运行

use probedb::ProbeDB;

fn main() {
    println!("🚀 ProbeDB v0.1.0 — AI内建数据库");
    println!("嵌入式模式，直接执行SQL\n");

    let mut db = ProbeDB::new();

    // 建表 + 插入 + 查询 演示
    let sqls = vec![
        "CREATE TABLE users (id INTEGER, name TEXT, age INTEGER)",
        "INSERT INTO users (id, name, age) VALUES (1, 'alice', 30)",
        "INSERT INTO users (id, name, age) VALUES (2, 'bob', 25)",
        "SELECT id, name, age FROM users",
        "CREATE TABLE items (id INTEGER, embedding VECTOR(3))",
        "INSERT INTO items (id, embedding) VALUES (1, '[0.1,0.2,0.3]')",
        "SELECT id, embedding FROM items",
        // 数据类型补全演示：BOOLEAN / DATE / TIME
        "CREATE TABLE memories (id INTEGER, content TEXT, created_date DATE, created_time TIME, pinned BOOLEAN)",
        "INSERT INTO memories (id, content, created_date, created_time, pinned) \
         VALUES (1, '上周和坦哥讨论了ProbeDB定位', '2026-09-10', '14:30', true), \
                (2, '会议纪要待整理', '2026-06-20', '09:00', false), \
                (3, 'HAVING子句已完成', '2026-09-15', '19:45', true)",
        "SELECT id, content, created_date, pinned FROM memories WHERE created_date >= '2026-09-01' AND pinned = true",
        "SELECT id, content, pinned FROM memories ORDER BY pinned DESC",
        "SELECT id, content, created_date FROM memories ORDER BY created_date DESC",
        "SELECT pinned, COUNT(*) FROM memories GROUP BY pinned",
        "INSERT INTO memories (id, content, created_date, created_time, pinned) VALUES (4, '非法日期', '2026-02-30', '10:00', false)",
    ];

    for sql in sqls {
        println!("▶ 执行: {}", sql);
        match db.execute(sql) {
            Ok(result) => println!("{}\n", result),
            Err(e) => println!("❌ 错误: {}\n", e),
        }
    }
}
