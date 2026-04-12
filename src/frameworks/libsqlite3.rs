use crate::dyld::{export_c_func, FunctionExports, HostDylib};
use crate::mem::{ConstPtr, MutPtr};
use crate::Environment;
use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::Mutex;

const SQLITE_OK: u32 = 0;
const SQLITE_ERROR: u32 = 1;

lazy_static::lazy_static! {
    static ref SQLITE_CONNECTIONS: Mutex<HashMap<u32, Connection>> = Mutex::new(HashMap::new());
    static ref NEXT_HANDLE: Mutex<u32> = Mutex::new(0x8000_0000);
}

// int sqlite3_open(const char *filename, sqlite3 **ppDb);
pub fn sqlite3_open(env: &mut Environment, filename_ptr: u32, ppDb: u32) -> u32 {
    // Читаем путь из памяти гостя вручную, байт за байтом, до нуль-терминатора
    let mut filename_bytes = Vec::new();
    let mut current_addr = filename_ptr;
    loop {
        // Используем ConstPtr для безопасного чтения
        let ptr: ConstPtr<u8> = ConstPtr::from_bits(current_addr);
        let byte: u8 = env.mem.read(ptr);
        if byte == 0 { 
            break; // Конец C-строки
        }
        filename_bytes.push(byte);
        current_addr += 1;
    }
    
    // Конвертируем прочитанные байты в строку Rust
    let filename = String::from_utf8_lossy(&filename_bytes).into_owned();

    // Заменяем слэши, чтобы файл создавался в локальной директории эмулятора безопасно
    let safe_name = filename.replace("/", "_");
    // Сохраняем базу в директорию приложения
    let path = format!("/storage/emulated/0/Android/data/org.touchhle.android.unofficial/files/touchHLE_apps/{}", safe_name);

    match Connection::open(&path) {
        Ok(conn) => {
            let mut handles = SQLITE_CONNECTIONS.lock().unwrap();
            let mut next_id = NEXT_HANDLE.lock().unwrap();
            
            let handle = *next_id;
            *next_id += 4;
            handles.insert(handle, conn);
            
            // Пишем handle обратно в память по адресу ppDb (тут нужен MutPtr)
            let pp_db_ptr: MutPtr<u32> = MutPtr::from_bits(ppDb);
            env.mem.write(pp_db_ptr, handle);
            
            SQLITE_OK
        }
        Err(e) => {
            println!("libsqlite3: Failed to open DB: {}", e);
            SQLITE_ERROR
        }
    }
}

pub fn sqlite3_open_v2(env: &mut Environment, filename_ptr: u32, ppDb: u32) -> u32 {
    // Читаем путь из памяти гостя вручную, байт за байтом, до нуль-терминатора
    let mut filename_bytes = Vec::new();
    let mut current_addr = filename_ptr;
    loop {
        // Используем ConstPtr для безопасного чтения
        let ptr: ConstPtr<u8> = ConstPtr::from_bits(current_addr);
        let byte: u8 = env.mem.read(ptr);
        if byte == 0 { 
            break; // Конец C-строки
        }
        filename_bytes.push(byte);
        current_addr += 1;
    }
    
    // Конвертируем прочитанные байты в строку Rust
    let filename = String::from_utf8_lossy(&filename_bytes).into_owned();

    // Заменяем слэши, чтобы файл создавался в локальной директории эмулятора безопасно
    let safe_name = filename.replace("/", "_");
    // Сохраняем базу в директорию приложения
    let path = format!("/storage/emulated/0/Android/data/org.touchhle.android.unofficial/files/touchHLE_apps/{}", safe_name);

    match Connection::open(&path) {
        Ok(conn) => {
            let mut handles = SQLITE_CONNECTIONS.lock().unwrap();
            let mut next_id = NEXT_HANDLE.lock().unwrap();
            
            let handle = *next_id;
            *next_id += 4;
            handles.insert(handle, conn);
            
            // Пишем handle обратно в память по адресу ppDb (тут нужен MutPtr)
            let pp_db_ptr: MutPtr<u32> = MutPtr::from_bits(ppDb);
            env.mem.write(pp_db_ptr, handle);
            
            SQLITE_OK
        }
        Err(e) => {
            println!("libsqlite3: Failed to open DB: {}", e);
            SQLITE_ERROR
        }
    }
}

// int sqlite3_close(sqlite3 *pDb);
pub fn sqlite3_close(_env: &mut Environment, p_db: u32) -> u32 {
    let mut handles = SQLITE_CONNECTIONS.lock().unwrap();
    if handles.remove(&p_db).is_some() {
        SQLITE_OK
    } else {
        SQLITE_ERROR
    }
}

pub fn sqlite3_close_v2(_env: &mut Environment, p_db: u32) -> u32 {
    let mut handles = SQLITE_CONNECTIONS.lock().unwrap();
    if handles.remove(&p_db).is_some() {
        SQLITE_OK
    } else {
        SQLITE_ERROR
    }
}

// Экспортируем функции через макрос из dyld.rs
pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(sqlite3_open(_, _)),
    export_c_func!(sqlite3_close(_)),
    export_c_func!(sqlite3_open_v2(_, _)),
    export_c_func!(sqlite3_close_v2(_)),
];

// Собираем всё в HostDylib, как требует архитектура dyld
pub const DYLIB: HostDylib = HostDylib {
    path: "/usr/lib/libsqlite3.dylib",
    aliases: &["/usr/lib/libsqlite3.0.dylib"],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[FUNCTIONS],
};
