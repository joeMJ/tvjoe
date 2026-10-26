//! Windows-Teil des Aufnehmens ohne Fenster: Rechner wach halten, Benachrichtigung.

/// Rechner während einer Aufnahme nicht in den Ruhezustand gehen lassen.
pub fn keep_awake(on: bool) {
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
    unsafe extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }
    let flags = if on {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    unsafe {
        SetThreadExecutionState(flags);
    }
}

/// Standardbenachrichtigung von Windows (Toast über PowerShell, ohne eigenes Fenster).
pub fn notify(summary: &str, body: &str) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('\'', "''")
    };
    let script = format!(
        "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
         [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null; \
         $x = New-Object Windows.Data.Xml.Dom.XmlDocument; \
         $x.LoadXml('<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>'); \
         $id = '{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe'; \
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($id).Show([Windows.UI.Notifications.ToastNotification]::new($x))",
        esc(summary),
        esc(body)
    );
    let _ = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

/// Aufgaben „\\tvjoe\\Aufnahme <Nummer>“ an den Plan angleichen: je geplanter Aufnahme eine
/// Aufgabe zur Startzeit (mit Vorlauf), nur bei Anmeldung (InteractiveToken, ohne
/// Administratorrechte und Kennwort, auf der VM geprüft), mit „zum Ausführen wecken“ und „nach
/// verpasstem Start sofort“. Aufgaben ohne Planung entfallen, laufende bleiben bis zum Ende.
/// Ausgeschaltet: alle entfernen.
pub fn sync_tasks(enabled: bool, timers: &[(i64, i64)]) {
    let Ok(exe) = std::env::current_exe() else { return };
    let want: Vec<String> = if enabled {
        timers
            .iter()
            .map(|(id, at)| format!("@{{n='Aufnahme {id}';id={id};at={at}}}"))
            .collect()
    } else {
        Vec::new()
    };
    let exe = exe.display().to_string().replace('\'', "''");
    let script = format!(
        "$ErrorActionPreference='SilentlyContinue'; $p='\\tvjoe\\'; $want=@({}); \
         $names=@($want | ForEach-Object {{ $_.n }}); \
         Get-ScheduledTask -TaskPath $p | Where-Object {{ $names -notcontains $_.TaskName -and $_.State -ne 'Running' }} | Unregister-ScheduledTask -Confirm:$false; \
         $u=\"$env:USERDOMAIN\\$env:USERNAME\"; \
         foreach($w in $want) {{ \
           $a=New-ScheduledTaskAction -Execute '{exe}' -Argument (\"--aufnahme \" + $w.id); \
           $t=New-ScheduledTaskTrigger -Once -At ([DateTimeOffset]::FromUnixTimeSeconds($w.at).LocalDateTime); \
           $pr=New-ScheduledTaskPrincipal -UserId $u -LogonType Interactive -RunLevel Limited; \
           $s=New-ScheduledTaskSettingsSet -WakeToRun -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Seconds 0) -MultipleInstances IgnoreNew; \
           Register-ScheduledTask -TaskName $w.n -TaskPath $p -Action $a -Trigger $t -Principal $pr -Settings $s -Force | Out-Null }}",
        want.join(",")
    );
    run_powershell(&script);
}

/// Aufnahme sofort als eigener Vorgang (Taste R): läuft weiter, wenn das Fenster schließt.
pub fn start_now(timer_id: i64) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe)
            .args(["--aufnahme", &timer_id.to_string()])
            .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .spawn();
    }
}

fn run_powershell(script: &str) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}
