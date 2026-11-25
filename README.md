# Molo Tracker

Molo is a Windows desktop app for reviewing your foreground app usage. Its interface and system tray use Anomaly’s Rune framework.

## Install on Windows

Download the x64 setup executable from [GitHub Releases](https://github.com/AmirMolotov/molo-tracker/releases/latest). Quit any running copy of Molo from the notification area before running Setup. Accept the [Terms of Use](installer/Terms.txt) and choose an install folder. Setup adds a Start menu shortcut. To add one to your desktop, select **Create a desktop shortcut**.

To uninstall, quit Molo from the notification area, then open **Settings → Apps → Installed apps** and choose **Uninstall** for **Molo Tracker**. Activity and settings remain in `%LOCALAPPDATA%\Molo Tracker`.

To build setup from source, install [Inno Setup 6](https://jrsoftware.org/inno-setup/downloads) and run `.\installer\build.ps1` with the Windows x64 MSVC Rust toolchain. The installer is written to `target\installer`.

## Run

Use stable Rust with the Windows MSVC target and the Visual Studio C++ build tools.

```powershell
cargo run --locked
```

Close the window to keep recording in the tray. Click the Molo icon in the Windows notification area to reopen it, or right-click the icon and choose **Quit Molo** to stop. The pause and play icon in the header pauses or resumes recording.

## Activity

**Activity** shows the selected day’s focus time, how it compares with the previous day, a chart by app, and a timetable of sessions. **Calendar** shows daily totals in a monthly grid. Select a date to open its activity.

Search is available in the app chart, timetable, Settings app folders, report app breakdown, and website reports. The query appears in large type and results filter as you type. Start typing to search the current screen's main list, or click a specific search field. Press Esc in a search field to clear it. App names and aliases match without regard to case. The timetable can also find recorded page titles and displayed times.

Chrome and Firefox sessions start collapsed in the timetable. Expand a session to see the focused tab titles and how long each title was visible. Molo reads the active browser window title and stores it locally. Earlier sessions can only show page titles if Molo recorded them at the time.

In **Settings**, **Timetable cutoff** starts at 5 seconds. Shorter sessions remain in daily totals and the usage chart. Set the cutoff to 0 to show all sessions. Select one app under **App aliases and groups** to rename it in the chart and timetable. Save a blank alias to restore the original name.

## Reports

In **Settings → App aliases and groups**, create a group such as **Games**. Ctrl-click to add or remove individual apps from the selection. Shift-click selects the range from your last selection anchor. Drag a selected app onto a group to move the whole selection, or enter a group name and choose **Move to group**. Drop apps on **Ungrouped** to remove their grouping. Each app can belong to one group, and reports apply current group membership to existing activity.

**Reports** shows focus time and session counts for apps and groups, with changes from the previous period. Choose **Daily**, **Weekly**, or **Monthly** and use the date controls to browse earlier periods. Weeks start on Monday. Ongoing periods show activity recorded so far compared with the full previous period. A focus session starts when Molo begins tracking a foreground app, including after switching back to an open app.

When activity from another PC is available, choose a PC under **Device** to filter the charts, website report, and previous-period comparison. Choose **All devices** to return to the combined report.

**Reports → Websites** groups recorded Chrome and Firefox focus time by main domain. Subdomains such as `news.bbc.co.uk` are combined under `bbc.co.uk`. The report lists each browser’s time, the combined total, and the change from the previous period. Molo reads the current browser address through Windows accessibility and saves only the domain for this report. This starts with new activity, and browser time still counts in the app and group reports when a domain cannot be read.

In **Reports → Websites**, choose **Set group** next to a domain. Use the same group name as an app to see both in the **Overview** group breakdown. Molo attributes recorded time on assigned websites to their group, and the browser still shows its full time in the app comparison. Website assignments apply to existing recordings and are saved on the PC where you set them.

Select a group in Settings to see its apps and use **Rename group** or **Delete group**. Deleting a group moves its apps to **Ungrouped** and keeps their activity history. The pie chart shows each group’s share of focus time. Select a group in the legend to see its apps. Report totals include all recorded sessions and take their time from the selected dates.

Molo samples the Windows foreground app about once per second. **Settings → Idle detection** defaults to 300 seconds without keyboard or mouse input. Molo shows **Idle** when the timeout elapses and resumes automatically after input. Enter 0 to disable idle detection. Usage is grouped by executable path. Apps that share a Windows host process can appear under the host name. Windows may hide the identity of some protected processes.

Under **Settings → Idle detection → App exceptions**, check an app to keep recording while it is in the foreground after the idle timeout. Uncheck it to restore the usual timeout. Manual pause still stops all recording.

The database is local at `%LOCALAPPDATA%\Molo Tracker\activity.sqlite3`. It stores executable paths, session times, aliases, groups, the cutoff, idle settings, browser tab titles, and recorded website domains.

## Sync across PCs

In **Settings → Account sync**, create an account, then sign in with the same username and password on each PC. Molo syncs each PC's name and activity, including existing history and recorded browser titles and domains. Activity, Calendar, and Reports count overlapping device time once, assigning each overlap to the most recently started session. The timetable keeps the original sessions with device tags. **Reports → Time by device** shows the time recorded on each PC, including overlaps.

The account also stores app groups, group membership, aliases, the timetable cutoff, and idle settings. Molo matches app settings by executable name across computers. Edits to different settings merge, and when two PCs change the same setting before syncing, the server keeps the first saved change. The server URL and manual pause stay per PC.

The per-Windows-user config is `%LOCALAPPDATA%\Molo Tracker\sync.toml`. Molo creates it on first run. To change servers, edit the file, quit Molo from the tray, and reopen it. Sign in to the account on the new server to begin syncing there.

```toml
server_url = "https://tracker.momentummania.com"
```

Tracking continues offline. Each PC queues local session changes in SQLite. Retries use the same device and session IDs, and each successful download advances a server revision cursor. After a failed sync, Molo waits 30 seconds, then doubles the delay after each consecutive failure up to 5 minutes. A successful sync resets the delay. Sign out to remove downloaded activity from this PC while retaining its own recordings.

The sync server is in `deploy/sync`. On this K3s cluster, `kubectl apply -k deploy/sync` deploys it in the `tracker` namespace. The deployment uses a single replica with a persistent SQLite volume, and Traefik serves the sync API over HTTPS at `tracker.momentummania.com`. Point that hostname at the cluster ingress. For a different cluster, set the hostname and TLS configuration in the manifest, back up the persistent volume, and use the corresponding HTTPS URL in each client's `sync.toml`.

## Check the interface

```powershell
cargo test --locked
cargo run --locked -- --scene populated --size 1120x820 --snap "$env:TEMP/molo-activity.png"
cargo run --locked -- --scene aliases --size 390x844 --snap "$env:TEMP/molo-aliases.png"
cargo run --locked -- --scene calendar --size 1120x820 --snap "$env:TEMP/molo-calendar.png"
cargo run --locked -- --scene reports --size 1120x980 --snap "$env:TEMP/molo-reports.png"
```

Rune’s preview scenes use an in-memory database and do not record desktop activity.
