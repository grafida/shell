# Grafida Shell — development tooling. Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
# A desktop-session helper (grafida/grafida#91), run by `scripts/dev/winvm.sh desktop VM <this>`.
# Prints the clipboard's plain text as JSON (a string, or null when it holds no text), so a
# newline or a non-ASCII character survives the trip back.
Add-Type -AssemblyName System.Windows.Forms
$text = if ([System.Windows.Forms.Clipboard]::ContainsText()) { [System.Windows.Forms.Clipboard]::GetText() } else { $null }
ConvertTo-Json -InputObject $text -Compress
