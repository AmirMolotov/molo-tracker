# Molo Tracker

Molo is a Windows desktop app for reviewing your foreground app usage. Its interface and system tray use Anomaly’s Rune framework.

## Run

Use stable Rust with the Windows MSVC target and the Visual Studio C++ build tools.

```powershell
cargo run --locked
```

Close the window to keep recording in the tray. Click the Molo icon in the Windows notification area to reopen it, or right-click the icon and choose **Quit Molo** to stop. The pause and play icon in the header pauses or resumes recording.

## Activity

**Activity** shows the selected day’s focus time, how it compares with the previous day, a chart by app, and a timetable of sessions. **Calendar** shows daily totals in a monthly grid. Select a date to open its activity.

Search is available in the app chart, timetable, Settings app folders, report app breakdown, and website reports. App names and aliases match without regard to case. The timetable can also find recorded page titles and displayed times.

Chrome and Firefox sessions start collapsed in the timetable. Expand a session to see the focused tab titles and how long each title was visible. Molo reads the active browser window title and stores it locally. Earlier sessions can only show page titles if Molo recorded them at the time.

In **Settings**, **Timetable cutoff** starts at 5 seconds. Shorter sessions remain in daily totals and the usage chart. Set the cutoff to 0 to show all sessions. Select one app under **App aliases and groups** to rename it in the chart and timetable. Save a blank alias to restore the original name.

## Reports

In **Settings → App aliases and groups**, create a folder such as **Games**. Ctrl-click to add or remove individual apps from the selection. Shift-click selects the range from your last selection anchor. Drag a selected app onto a folder to move the whole selection, or enter a folder name and choose **Move to folder**. Open a folder to see its apps. Drop apps on **Ungrouped** to remove their grouping. Each app can belong to one folder, and reports apply current group membership to existing activity.

**Reports** shows focus time and session counts for apps and groups, with changes from the previous period. Choose **Daily**, **Weekly**, or **Monthly** and use the date controls to browse earlier periods. Weeks start on Monday. Ongoing periods show activity recorded so far compared with the full previous period. A focus session starts when Molo begins tracking a foreground app, including after switching back to an open app.

**Reports → Websites** groups recorded Chrome and Firefox focus time by main domain. Subdomains such as `news.bbc.co.uk` are combined under `bbc.co.uk`. The report lists each browser’s time, the combined total, and the change from the previous period. Molo reads the current browser address through Windows accessibility and saves only the domain for this report. This starts with new activity, and browser time still counts in the app and group reports when a domain cannot be read.

The pie chart shows each group’s share of focus time. Select a group in the legend to see its apps. Report totals include all recorded sessions and take their time from the selected dates.

Molo samples the Windows foreground app about once per second. It stops attributing time after five minutes without keyboard or mouse input. Usage is grouped by executable path. Apps that share a Windows host process can appear under the host name. Windows may hide the identity of some protected processes.

The database is local at `%LOCALAPPDATA%\Molo Tracker\activity.sqlite3`. It stores executable paths, session times, aliases, groups, the cutoff, browser tab titles, and recorded website domains.

## Check the interface

```powershell
cargo test --locked
cargo run --locked -- --scene populated --size 1120x820 --snap "$env:TEMP/molo-activity.png"
cargo run --locked -- --scene aliases --size 390x844 --snap "$env:TEMP/molo-aliases.png"
cargo run --locked -- --scene calendar --size 1120x820 --snap "$env:TEMP/molo-calendar.png"
cargo run --locked -- --scene reports --size 1120x980 --snap "$env:TEMP/molo-reports.png"
```

Rune’s preview scenes use an in-memory database and do not record desktop activity.
