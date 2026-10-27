# A still, full-screen gradient shown behind the launchers while benchmarking,
# so nothing changing on screen bleeds through their translucent backdrops.
param([int]$Seconds = 300)
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -Name Dpi -Namespace Bench -MemberDefinition '[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();'
[Bench.Dpi]::SetProcessDPIAware() | Out-Null

$form = New-Object System.Windows.Forms.Form
$form.FormBorderStyle = "None"
$form.WindowState = "Maximized"
$form.ShowInTaskbar = $false
$form.Text = "bench-backdrop"
$form.Add_Paint({
    param($sender, $e)
    $rect = $sender.ClientRectangle
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush($rect, [System.Drawing.Color]::FromArgb(30, 27, 75), [System.Drawing.Color]::FromArgb(14, 116, 144), 35)
    $e.Graphics.FillRectangle($brush, $rect)
})
$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = $Seconds * 1000
$timer.Add_Tick({ $form.Close() })
$timer.Start()
[System.Windows.Forms.Application]::Run($form)
