# Grafida Shell — development tooling. Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
# A desktop-session helper (grafida/grafida#91), run by `scripts/dev/winvm.sh desktop VM <this> [args]`.
# Switches Windows' app and system mode (Settings ▸ Personalisation ▸ Colours ▸ Choose your mode)
# and tells running programs, as the Settings app does.
param([Parameter(Mandatory)] [ValidateSet('light', 'dark')] [string] $Mode)
$v = if ($Mode -eq 'light') { 1 } else { 0 }
$k = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
Set-ItemProperty $k AppsUseLightTheme $v -Type DWord
Set-ItemProperty $k SystemUsesLightTheme $v -Type DWord
Add-Type -Namespace W -Name U -MemberDefinition '[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern System.IntPtr SendMessageTimeout(System.IntPtr h, uint m, System.UIntPtr w, string l, uint f, uint t, out System.UIntPtr r);'
$r = [UIntPtr]::Zero
[void][W.U]::SendMessageTimeout([IntPtr]0xffff, 0x001A, [UIntPtr]::Zero, 'ImmersiveColorSet', 2, 3000, [ref]$r)
"mode: $Mode"
