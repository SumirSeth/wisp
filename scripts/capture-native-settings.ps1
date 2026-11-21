param([Parameter(Mandatory=$true)][string]$Exe,[Parameter(Mandatory=$true)][string]$Name,[switch]$Native,[int]$Wheel=0,[ValidateSet("general","appearance","search","about")][string]$Page="general",[switch]$Welcome,[switch]$Advanced,[switch]$Picker,[ValidateSet("none","latest","available","error")][string]$UpdateState="none")
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class SettingsWindowCapture {
 [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left,Top,Right,Bottom; }
 public delegate bool EnumProc(IntPtr hwnd, IntPtr param);
 [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb,IntPtr p);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd,out uint pid);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd,out Rect rect);
 [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hwnd,uint message,IntPtr wparam,IntPtr lparam);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd,IntPtr dc,uint flags);
}
'@
$shots=Join-Path $PSScriptRoot '../target/native-settings-shots'
New-Item -ItemType Directory -Force $shots | Out-Null
foreach($theme in 'dark','light') {
 $root=Join-Path $shots "$Name-$theme-data"
 $data=Join-Path $root 'wisp';New-Item -ItemType Directory -Force $data | Out-Null
 $config=Join-Path $data 'config.toml'
 $text=Get-Content (Join-Path $PSScriptRoot '../src/default-config.toml') -Raw
 $text=$text -replace '(?m)^theme\s*=.*$',"theme = `"$theme`""
 if($Page -eq 'appearance') {$text=$text -replace '(?m)^dock_color\s*=.*$', 'dock_color = "#1E3A8ABF"'}
 $text=$text -replace '(?m)^check_for_updates\s*=.*$','check_for_updates = false'
 Set-Content -LiteralPath $config -Value $text
 $start=New-Object Diagnostics.ProcessStartInfo
 $start.FileName=(Resolve-Path $Exe).Path
 $start.UseShellExecute=$false
 $start.RedirectStandardError=$true
 $start.EnvironmentVariables['APPDATA']=$root
 $start.EnvironmentVariables['WISP_RELEASE_URL']='file:///nonexistent-native-settings-fixture.json'
 $start.Arguments=if($Native) {"--settings --config `"$config`" --page $Page"} else {"--page $Page"}
 if($Welcome) {$start.Arguments += " --welcome"}
 if($UpdateState -ne 'none') {
  $fixture=Join-Path $root 'release.json'
  $tag=if($UpdateState -eq 'latest'){'v0.0.0'}else{'v99.0.0'}
  if($UpdateState -eq 'error') { Set-Content $fixture 'broken release response' } else {
   @{tag_name=$tag;html_url='https://example.invalid/fixture';assets=@()} | ConvertTo-Json | Set-Content $fixture
  }
  $start.EnvironmentVariables['WISP_RELEASE_URL']="file://$fixture"
  $start.EnvironmentVariables['WISP_SETTINGS_FIXTURE']='1'
 }
 $process=[Diagnostics.Process]::Start($start)
 try {
  Start-Sleep -Seconds $(if($Native) {1} else {12})
  $script:settingsCaptureHandle=[IntPtr]::Zero
  $ownedProcess=$process.Id
  [SettingsWindowCapture]::EnumWindows({param($h,$p) [uint32]$windowProcess=0;[void][SettingsWindowCapture]::GetWindowThreadProcessId($h,[ref]$windowProcess);if($windowProcess -eq $ownedProcess -and [SettingsWindowCapture]::IsWindowVisible($h)) {$script:settingsCaptureHandle=$h;return $false};return $true},[IntPtr]::Zero) | Out-Null
  if($script:settingsCaptureHandle -eq [IntPtr]::Zero) {if(!$process.HasExited) {$process.Kill();$process.WaitForExit()};throw "No Settings window for process $ownedProcess (exited=$($process.HasExited)). $($process.StandardError.ReadToEnd())"}
  if($Advanced -and $Page -eq 'appearance') {
   # Messages go only to the window owned by the child process above.
   $x=400;$y=if($Native){290}else{250}
   $point=[IntPtr](($y -shl 16) -bor $x)
   [void][SettingsWindowCapture]::PostMessageW($script:settingsCaptureHandle,0x0201,[IntPtr]1,$point)
   [void][SettingsWindowCapture]::PostMessageW($script:settingsCaptureHandle,0x0202,[IntPtr]0,$point)
   Start-Sleep -Milliseconds 300
  }
  if($Picker -and $Native -and $Page -eq 'appearance') {
   $point=[IntPtr]((480 -shl 16) -bor 400)
   [void][SettingsWindowCapture]::PostMessageW($script:settingsCaptureHandle,0x0201,[IntPtr]1,$point)
   [void][SettingsWindowCapture]::PostMessageW($script:settingsCaptureHandle,0x0202,[IntPtr]0,$point)
   Start-Sleep -Milliseconds 300
  }
  # -Wheel N scrolls the window down N notches before the capture.
  for($k=0;$k -lt $Wheel;$k++) {[void][SettingsWindowCapture]::PostMessageW($script:settingsCaptureHandle,0x020A,[IntPtr](-120 -shl 16),[IntPtr]::Zero)}
  if($Wheel) {Start-Sleep -Milliseconds 400}
  $rect=New-Object SettingsWindowCapture+Rect
  [void][SettingsWindowCapture]::GetWindowRect($script:settingsCaptureHandle,[ref]$rect)
  $bitmap=New-Object Drawing.Bitmap(($rect.Right-$rect.Left),($rect.Bottom-$rect.Top))
  $graphics=[Drawing.Graphics]::FromImage($bitmap)
  $dc=$graphics.GetHdc()
  try {$ok=[SettingsWindowCapture]::PrintWindow($script:settingsCaptureHandle,$dc,2)} finally {$graphics.ReleaseHdc($dc)}
  $path=Join-Path $shots "$Name-$theme.png"
  $bitmap.Save($path,[Drawing.Imaging.ImageFormat]::Png)
  $graphics.Dispose();$bitmap.Dispose()
  Write-Output "$path (PrintWindow=$ok)"
 } finally {
  if(!$process.HasExited -and $script:settingsCaptureHandle -ne [IntPtr]::Zero) {
   [void][SettingsWindowCapture]::PostMessageW($script:settingsCaptureHandle,0x10,[IntPtr]::Zero,[IntPtr]::Zero)
   $closed=$process.WaitForExit(3000)
   Write-Output "Settings closed through WM_CLOSE: $closed"
  }
  if(!$process.HasExited) {$process.Kill();$process.WaitForExit()};$process.Dispose()
 }
}

# Keep the original pixels; put the two window-only captures beside each other.
if($Native) {
 foreach($theme in 'dark','light') {
  $reference=Join-Path $shots "old-$Page-$(if($Wheel){'scrolled'}else{'top'})-$theme.png"
  if($Welcome) {$reference=Join-Path $shots "old-welcome-$theme.png"}
  $current=Join-Path $shots "$Name-$theme.png"
  if((Test-Path $reference) -and (Test-Path $current)) {
   $old=[Drawing.Image]::FromFile($reference);$new=[Drawing.Image]::FromFile($current)
   try {
    $comparison=New-Object Drawing.Bitmap(($old.Width+$new.Width),([Math]::Max($old.Height,$new.Height)+32))
    $g=[Drawing.Graphics]::FromImage($comparison)
    $font=New-Object Drawing.Font('Segoe UI',12)
    try {
     $g.Clear([Drawing.Color]::FromArgb(32,32,32))
     $g.DrawString('Current Settings (GPUI)',$font,[Drawing.Brushes]::White,8,4)
     $g.DrawString('Native Settings (Direct2D)',$font,[Drawing.Brushes]::White,($old.Width+8),4)
     $g.DrawImageUnscaled($old,0,32);$g.DrawImageUnscaled($new,$old.Width,32)
     $comparisonName=if($Picker){"comparison-appearance-picker-$theme.png"}elseif($Welcome){"comparison-welcome-$theme.png"}else{"comparison-$Page-$(if($Wheel){'scrolled'}else{'top'})-$theme.png"}
     $comparison.Save((Join-Path $shots $comparisonName),[Drawing.Imaging.ImageFormat]::Png)
    } finally {$font.Dispose();$g.Dispose();$comparison.Dispose()}
   } finally {$old.Dispose();$new.Dispose()}
  }
 }
}
