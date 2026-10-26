# Installs the built app as someone would and starts it with a library to
# show (thousands of awkward and deliberately broken test images, faces,
# pets and a movie) and smart search, faces and pets switched on. Fails if
# it isn't still running a few minutes later. Either way it prints the
# app's log and any crash Windows recorded, and saves screenshots.
#
#   pwsh scripts/smoke-test.ps1 [-Installer <setup.exe>] [-Seconds 180] [-Screens <dir>]

param(
    [string]$Installer = (Get-ChildItem 'target/release/bundle/nsis/*.exe' | Select-Object -First 1).FullName,
    [int]$Seconds = 180,
    [string]$Screens = 'smoke-screens'
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$id = 'app.fujicloud.desktop'
$data = Join-Path $env:LOCALAPPDATA $id
$config = Join-Path $env:APPDATA $id
$temp = [IO.Path]::GetTempPath()
$photos = Join-Path $temp 'fujicloud-smoke-photos'
$work = Join-Path $temp 'fujicloud-smoke-download'
New-Item -ItemType Directory -Force -Path $photos, $work, $Screens | Out-Null

# ---- photos ---------------------------------------------------------------
# Pillow's and pillow-heif's test images, many of them odd or broken on
# purpose.
$sdists = @(
    'https://files.pythonhosted.org/packages/ce/a4/9cc30690a394a8305e8be4c9036aa16d0dfddadd09079860b582f562496d/pillow-13.0.0.tar.gz',
    'https://files.pythonhosted.org/packages/44/c1/82145984920ca055675af2c2795bd30da6f7461215c41f3c1eacb3d66353/pillow_heif-1.8.1.tar.gz'
)
foreach ($url in $sdists) {
    $tgz = Join-Path $work (Split-Path $url -Leaf)
    Invoke-WebRequest $url -OutFile $tgz
    tar -xzf $tgz -C $work
}
Copy-Item -Recurse -Force (Join-Path $work 'pillow-13.0.0/Tests/images') (Join-Path $photos 'pillow')
Copy-Item -Recurse -Force (Join-Path $work 'pillow_heif-1.8.1/tests/images') (Join-Path $photos 'heif')
# People, pets and a movie, as the real-model tests use.
$singles = @(
    'https://raw.githubusercontent.com/ageitgey/face_recognition/master/examples/obama.jpg',
    'https://raw.githubusercontent.com/ageitgey/face_recognition/master/examples/obama2.jpg',
    'https://raw.githubusercontent.com/ageitgey/face_recognition/master/examples/biden.jpg',
    'https://raw.githubusercontent.com/ageitgey/face_recognition/master/examples/two_people.jpg',
    'https://raw.githubusercontent.com/EliSchwartz/imagenet-sample-images/master/n02123045_tabby.JPEG',
    'https://raw.githubusercontent.com/EliSchwartz/imagenet-sample-images/master/n02123394_Persian_cat.JPEG',
    'https://raw.githubusercontent.com/EliSchwartz/imagenet-sample-images/master/n02099601_golden_retriever.JPEG',
    'https://raw.githubusercontent.com/web-platform-tests/wpt/master/media/movie_5.mp4'
)
$mine = Join-Path $photos 'camera'
New-Item -ItemType Directory -Force -Path $mine | Out-Null
foreach ($url in $singles) {
    Invoke-WebRequest $url -OutFile (Join-Path $mine (Split-Path $url -Leaf))
}
$count = (Get-ChildItem -Recurse -File $photos).Count
Write-Host "$count files in $photos"

# ---- install and set up ---------------------------------------------------
Start-Process $Installer -ArgumentList '/S' -Wait
$exe = Get-ChildItem (Join-Path $env:LOCALAPPDATA 'Fujicloud') -Recurse -Filter '*.exe' |
    Where-Object { $_.Name -notmatch 'uninstall' } | Select-Object -First 1
if (-not $exe) { throw 'the installer did not install Fujicloud' }
Write-Host "installed $($exe.FullName)"

New-Item -ItemType Directory -Force -Path $config, $data | Out-Null
@{ smartSearch = $true; smartGpu = $true; faces = $true; pets = $true } |
    ConvertTo-Json | Set-Content (Join-Path $config 'settings.json')
# The library starts out knowing the folder, as if it had been added.
python -c @"
import sqlite3, sys
c = sqlite3.connect(sys.argv[1])
c.execute('CREATE TABLE IF NOT EXISTS sources (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, label TEXT NOT NULL)')
c.execute('INSERT OR IGNORE INTO sources (path, label) VALUES (?, ?)', (sys.argv[2], 'Smoke test'))
c.commit()
"@ (Join-Path $data 'library.db') $photos

# ---- run ------------------------------------------------------------------
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
function Save-Screen([string]$name) {
    try {
        $bounds = [Windows.Forms.Screen]::PrimaryScreen.Bounds
        $bmp = New-Object Drawing.Bitmap $bounds.Width, $bounds.Height
        $g = [Drawing.Graphics]::FromImage($bmp)
        $g.CopyFromScreen($bounds.Location, [Drawing.Point]::Empty, $bounds.Size)
        $bmp.Save((Join-Path $Screens "$name.png"))
        $g.Dispose(); $bmp.Dispose()
    } catch { Write-Host "no screenshot: $_" }
}

$started = Get-Date
$app = Start-Process $exe.FullName -PassThru
$null = $app.Handle  # keeps the exit code readable
$next = 20
while (((Get-Date) - $started).TotalSeconds -lt $Seconds -and -not $app.HasExited) {
    Start-Sleep -Seconds 2
    $t = [int]((Get-Date) - $started).TotalSeconds
    if ($t -ge $next) { Save-Screen ('{0:D3}s' -f $next); $next += 40 }
}
$crashed = $app.HasExited
$ran = [int]((Get-Date) - $started).TotalSeconds
if ($crashed) {
    Write-Host ("::error::Fujicloud quit after {0} s with exit code 0x{1:X8}" -f $ran, $app.ExitCode)
} else {
    Save-Screen 'last'
    Stop-Process -Id $app.Id -Force
    Write-Host "Fujicloud was still running after $ran s"
}

# ---- report ---------------------------------------------------------------
python -c @"
import sqlite3, sys
c = sqlite3.connect(sys.argv[1])
q = lambda s: c.execute(s).fetchone()[0]
print('photos', q('SELECT count(*) FROM photos'), '| searched for faces', q('SELECT count(*) FROM face_scans'),
      '| for pets', q('SELECT count(*) FROM pet_scans'), '| faces', q('SELECT count(*) FROM faces'))
"@ (Join-Path $data 'library.db')
$thumbs = (Get-ChildItem -Recurse -File (Join-Path $data 'cache') -ErrorAction SilentlyContinue).Count
Write-Host "thumbnails cached: $thumbs"
Write-Host '---- fujicloud.log'
Get-Content (Join-Path $data 'logs/fujicloud.log') -ErrorAction SilentlyContinue
Write-Host '---- crashes Windows recorded'
Get-WinEvent -FilterHashtable @{ LogName = 'Application'; StartTime = $started } -ErrorAction SilentlyContinue |
    Where-Object { $_.Message -match 'fujicloud' } |
    ForEach-Object { "$($_.TimeCreated) $($_.ProviderName): $($_.Message)" }
if ($crashed) { exit 1 }
