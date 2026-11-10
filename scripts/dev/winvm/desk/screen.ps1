# Grafida Shell — development tooling. Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
# A desktop-session helper (grafida/grafida#91), run by `scripts/dev/winvm.sh desktop VM <this> [args]`.
# The whole desktop as a PNG: what is on the screen, native menus and dialogs included (a webview
# capture shows the page only).
param([Parameter(Mandatory)] [string] $Path)
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$b = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
$bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
"saved $Path ($($b.Width)x$($b.Height))"
