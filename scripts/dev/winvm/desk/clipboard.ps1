# Grafida Shell — development tooling. Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
# A desktop-session helper (grafida/grafida#91), run by `scripts/dev/winvm.sh desktop VM <this> [args]`.
# Puts formatted text on the clipboard, as copying from WordPad does: HTML and plain text.
param([Parameter(Mandatory)] [string] $Html, [Parameter(Mandatory)] [string] $Text)
Add-Type -AssemblyName System.Windows.Forms
$data = New-Object System.Windows.Forms.DataObject
$pre = '<html><body><!--StartFragment-->'; $post = '<!--EndFragment--></body></html>'
$header = 'Version:0.9`r`nStartHTML:{0:D10}`r`nEndHTML:{1:D10}`r`nStartFragment:{2:D10}`r`nEndFragment:{3:D10}`r`n'
$len = { param($s) [Text.Encoding]::UTF8.GetByteCount($s) }
$h0 = (& $len ($header -f 0, 0, 0, 0))
$start = $h0; $fs = $start + (& $len $pre); $fe = $fs + (& $len $Html); $end = $fe + (& $len $post)
$cf = ($header -f $start, $end, $fs, $fe) + $pre + $Html + $post
$data.SetData([System.Windows.Forms.DataFormats]::Html, $cf)
$data.SetData([System.Windows.Forms.DataFormats]::UnicodeText, $Text)
[System.Windows.Forms.Clipboard]::SetDataObject($data, $true)
"clipboard: html + text"
