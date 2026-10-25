//! Windows: Starter und Oberfläche sind Fensterprogramme (kein schwarzes Konsolenfenster beim
//! Start aus dem Startmenü). Aus einer Konsole aufgerufen, hängen sie sich an deren Fenster, damit
//! Ausgaben wie `tvjoe --check` oder die Zeitmessung (`TVJOE_TRACE`) dort erscheinen.

/// An die Konsole des aufrufenden Prozesses hängen, falls es eine gibt (sonst ohne Wirkung).
pub fn attach_parent() {
    #[cfg(windows)]
    {
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        unsafe extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
        }
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}
