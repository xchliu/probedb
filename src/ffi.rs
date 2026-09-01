// ProbeDB C FFI — 跨语言进程内调用层（Hermes 接入的第一版接口）
//
// 设计约束：
//   1. 零外部依赖 —— 只用 std，保持"编译必须离线可完成"铁律
//   2. 嵌入式模式 —— 类似 SQLite 用法，Python(ctypes)/C/其他语言直接调用
//   3. 内存所有权 —— Rust 侧分配的内存由 Rust 侧释放（probedb_free_string）
//   4. panic 安全 —— 所有入口 catch_unwind，绝不让 panic 跨 FFI 边界
//
// 函数风格：SQLite C API 的简化版。所有错误通过 NULL 返回值 + probedb_last_error() 获取。

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::Mutex;

use crate::ProbeDB;

/// 数据库句柄（不透明指针，跨语言持有）
pub struct ProbeDBHandle {
    db: ProbeDB,
}

/// 全局错误信息（FFI 边界无法返回 Result，用此传递最后一条错误）
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());

fn set_last_error(msg: &str) {
    if let Ok(mut guard) = LAST_ERROR.lock() {
        *guard = msg.to_string();
    }
}

fn take_last_error() -> String {
    LAST_ERROR.lock().map(|g| g.clone()).unwrap_or_default()
}

/// 打开（或创建）数据库。
/// - path = NULL → 纯内存模式
/// - path = 路径  → 磁盘模式（自动加载快照 + 重放 WAL 崩溃恢复）
/// 成功返回句柄；失败返回 NULL（错误见 probedb_last_error）。
#[no_mangle]
pub extern "C" fn probedb_open(path: *const c_char) -> *mut ProbeDBHandle {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let db = if path.is_null() {
            ProbeDB::new()
        } else {
            let p = unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned();
            ProbeDB::open(&p)?
        };
        Ok::<_, String>(Box::into_raw(Box::new(ProbeDBHandle { db })))
    }));
    match result {
        Ok(Ok(handle)) => handle,
        Ok(Err(e)) => {
            set_last_error(&e);
            ptr::null_mut()
        }
        Err(_) => {
            set_last_error("panic in probedb_open");
            ptr::null_mut()
        }
    }
}

/// 执行 SQL，成功返回结果文本（Rust 侧分配，用 probedb_free_string 释放）；
/// 失败返回 NULL（错误见 probedb_last_error）。
#[no_mangle]
pub extern "C" fn probedb_execute(
    handle: *mut ProbeDBHandle,
    sql: *const c_char,
) -> *mut c_char {
    if handle.is_null() {
        set_last_error("null handle");
        return ptr::null_mut();
    }
    if sql.is_null() {
        set_last_error("null sql");
        return ptr::null_mut();
    }
    let h = unsafe { &mut *handle };
    let sql_str = unsafe { CStr::from_ptr(sql) }.to_string_lossy().into_owned();

    let result = catch_unwind(AssertUnwindSafe(|| h.db.execute(&sql_str)));
    match result {
        Ok(Ok(out)) => match CString::new(out) {
            Ok(c) => c.into_raw(),
            Err(_) => {
                set_last_error("result contains NUL byte");
                ptr::null_mut()
            }
        },
        Ok(Err(e)) => {
            set_last_error(&e);
            ptr::null_mut()
        }
        Err(_) => {
            set_last_error("panic in probedb_execute");
            ptr::null_mut()
        }
    }
}

/// 将当前状态原子保存到磁盘（先 flush WAL 再写快照）。
/// 返回 0 成功，-1 失败（错误见 probedb_last_error）。
#[no_mangle]
pub extern "C" fn probedb_persist(handle: *mut ProbeDBHandle) -> i32 {
    if handle.is_null() {
        set_last_error("null handle");
        return -1;
    }
    let h = unsafe { &mut *handle };
    let result = catch_unwind(AssertUnwindSafe(|| h.db.persist()));
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            set_last_error(&e);
            -1
        }
        Err(_) => {
            set_last_error("panic in probedb_persist");
            -1
        }
    }
}

/// 开启/关闭 WAL 批量模式。返回 0 成功，-1 失败。
#[no_mangle]
pub extern "C" fn probedb_set_batch_mode(handle: *mut ProbeDBHandle, enabled: i32) -> i32 {
    if handle.is_null() {
        set_last_error("null handle");
        return -1;
    }
    let h = unsafe { &mut *handle };
    let result = catch_unwind(AssertUnwindSafe(|| h.db.set_batch_mode(enabled != 0)));
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            set_last_error(&e);
            -1
        }
        Err(_) => {
            set_last_error("panic in probedb_set_batch_mode");
            -1
        }
    }
}

/// 取最后一条错误信息（Rust 侧分配，用 probedb_free_string 释放）。
/// 无错误时返回 NULL。
#[no_mangle]
pub extern "C" fn probedb_last_error() -> *mut c_char {
    let msg = take_last_error();
    if msg.is_empty() {
        return ptr::null_mut();
    }
    match CString::new(msg) {
        Ok(c) => c.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

/// 释放 Rust 侧分配的字符串（probedb_execute / probedb_last_error 的返回值）
#[no_mangle]
pub extern "C" fn probedb_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            drop(CString::from_raw(s));
        }
    }
}

/// 关闭数据库并释放句柄。
#[no_mangle]
pub extern "C" fn probedb_close(handle: *mut ProbeDBHandle) {
    if !handle.is_null() {
        unsafe {
            drop(Box::from_raw(handle));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn c(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    fn cstr_to_string(p: *mut c_char) -> String {
        assert!(!p.is_null(), "FFI 返回值不应为 NULL");
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        unsafe { probedb_free_string(p) };
        s
    }

    #[test]
    fn test_ffi_memory_mode_crud() {
        let db = probedb_open(ptr::null());
        assert!(!db.is_null(), "内存模式打开应成功: {}", cstr_to_string(probedb_last_error()));

        let sql = c("CREATE TABLE t (id INTEGER, name TEXT)");
        let r = probedb_execute(db, sql.as_ptr());
        assert!(!r.is_null());
        assert!(cstr_to_string(r).contains("创建成功"));

        let sql = c("INSERT INTO t (id, name) VALUES (1, 'alice')");
        let r = probedb_execute(db, sql.as_ptr());
        assert!(!r.is_null(), "插入失败: {}", cstr_to_string(probedb_last_error()));
        cstr_to_string(r);

        let sql = c("SELECT id, name FROM t");
        let r = probedb_execute(db, sql.as_ptr());
        assert!(cstr_to_string(r).contains("alice"));

        probedb_close(db);
    }

    #[test]
    fn test_ffi_error_propagation() {
        let db = probedb_open(ptr::null());
        assert!(!db.is_null());

        // 查询不存在的表 → 返回 NULL + last_error
        let sql = c("SELECT * FROM ghost");
        let r = probedb_execute(db, sql.as_ptr());
        assert!(r.is_null(), "应返回 NULL");
        let err = cstr_to_string(probedb_last_error());
        assert!(err.contains("ghost"), "错误应提到表名: {}", err);

        probedb_close(db);
    }

    #[test]
    fn test_ffi_null_guard() {
        // NULL 句柄 → 返回 NULL + 明确错误
        let r = probedb_execute(ptr::null_mut(), c("SELECT 1").as_ptr());
        assert!(r.is_null());
        let err = cstr_to_string(probedb_last_error());
        assert!(err.contains("null handle"), "错误应说明 NULL 句柄: {}", err);

        // NULL SQL → 返回 NULL + 明确错误
        let db = probedb_open(ptr::null());
        let r = probedb_execute(db, ptr::null());
        assert!(r.is_null());
        let err = cstr_to_string(probedb_last_error());
        assert!(err.contains("null sql"), "错误应说明 NULL SQL: {}", err);
        probedb_close(db);
    }

    #[test]
    fn test_ffi_persist_reopen() {
        let dir = std::env::temp_dir();
        let path = dir.join("probedb_ffi_persist_test.pdb");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}.wal", path.display()));
        let path_c = c(path.to_str().unwrap());

        // 写会话：建表+插入+persist
        let db = probedb_open(path_c.as_ptr());
        assert!(!db.is_null(), "打开失败: {}", cstr_to_string(probedb_last_error()));
        for sql in [
            "CREATE TABLE m (key TEXT, value TEXT)",
            "INSERT INTO m (key, value) VALUES ('k1', 'v1')",
            "INSERT INTO m (key, value) VALUES ('k2', 'v2')",
        ] {
            let s = c(sql);
            let r = probedb_execute(db, s.as_ptr());
            assert!(!r.is_null(), "{} 失败: {}", sql, cstr_to_string(probedb_last_error()));
            cstr_to_string(r);
        }
        assert_eq!(probedb_persist(db), 0, "persist 失败: {}", cstr_to_string(probedb_last_error()));
        probedb_close(db);

        // 读会话：重新打开 → 数据在
        let db2 = probedb_open(path_c.as_ptr());
        assert!(!db2.is_null(), "重开失败: {}", cstr_to_string(probedb_last_error()));
        let s = c("SELECT key, value FROM m ORDER BY key ASC");
        let r = probedb_execute(db2, s.as_ptr());
        let out = cstr_to_string(r);
        assert!(out.contains("k1") && out.contains("v2"), "持久化数据应可恢复: {}", out);
        probedb_close(db2);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}.wal", path.display()));
    }
}
