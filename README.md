# XCLayer

**Xbox Series X|S compatibility layer for Windows.**

XCLayer is a fork of [XCL](https://github.com/ivall/xcl) that enables compatible Xbox Series X|S GDK applications to run on Windows. Game x64 code executes natively, while console-specific system libraries and graphics interfaces are translated to their Windows equivalents. RDNA2 shaders are recompiled to HLSL and DXIL for Direct3D 12.

The fork focuses on stability, memory management, shader translation and runtime performance. Compatibility remains experimental; support varies by title, GPU and driver.

[Upstream project](https://github.com/ivall/xcl) · [Upstream Discord](https://discord.gg/xcl) · [Issues](https://github.com/w1spo/xclayer/issues) · [Builds](https://github.com/w1spo/xclayer/actions)

## Requirements

| Component | Requirement |
| --- | --- |
| Operating system | Windows 10 22H2 (KB5029331 or later), or Windows 11, x64 |
| Graphics | Direct3D 12 GPU with Shader Model 6.6 and mesh shader support |
| Drivers | Current graphics drivers supporting the required D3D12 features |
| Runtime | Windows Gaming Services (`xgameruntime.dll`) |
| Shader compiler | DXC 1.8 or newer recommended |
| Memory | Sufficient RAM, disk space and system-managed page file for the title |

AMD RDNA hardware is the primary development target upstream. NVIDIA GPUs use a Wave64-to-Wave32 translation path where applicable, which can affect performance and correctness. Hardware support does not guarantee game compatibility.

## Installation

### 1. Install the prerequisites

Install the following tools to build XCLayer from source:

- [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/) with the **Desktop development with C++** workload and a Windows SDK.
- [Rust](https://rustup.rs/) with the stable Windows MSVC toolchain.
- [Git](https://git-scm.com/download/win).
- [DirectX Shader Compiler](https://github.com/microsoft/DirectXShaderCompiler/releases) (if the Windows SDK does not provide a suitable DXC version).

Make sure Windows Gaming Services is installed through the Microsoft Store or Xbox app.

### 2. Build XCLayer

Open PowerShell and run:

```powershell
rustup toolchain install stable-x86_64-pc-windows-msvc
rustup default stable-x86_64-pc-windows-msvc

git clone https://github.com/w1spo/xclayer.git
cd xclayer
cargo build --release --workspace --locked
```

The executable will be generated at:

```text
target\release\xcl.exe
```

Prebuilt binaries, when published, can be found under [Releases](https://github.com/w1spo/xclayer/releases). Otherwise, build from source as described above.

### 3. Configure DirectX

If using a standalone DXC distribution, extract it and set the path to the directory containing `dxcompiler.dll` and `dxil.dll`:

```powershell
$env:XCL_DXC_DIR = 'C:\Tools\DXC\bin\x64'
```

Replace the example directory with the actual location. These PowerShell environment settings apply to the current session.

On Windows 10, the [Direct3D 12 Agility SDK](https://devblogs.microsoft.com/directx/directx12agility/) is required for Shader Model 6.6. Place the compatible `D3D12Core.dll` in a `D3D12` directory alongside `xcl.exe`, or set `XCL_D3D12_DIR` to the directory containing it. Include the matching SDK layers DLL if enabling the D3D12 debug layer.

## Usage

XCLayer requires legally obtained, unpacked game files. It does not decrypt packages or bypass DRM.

Launch `xcl.exe` to open the game selection menu, or use PowerShell from the repository directory:

```powershell
.\target\release\xcl.exe inspect 'D:\Games\Example\game.exe'
.\target\release\xcl.exe run 'D:\Games\Example'
```

The original game directory remains unchanged. Saves, configuration, shader caches and other writable data are stored separately.

### Controls

An XInput-compatible controller is recommended. Keyboard and mouse input can also emulate a controller; configure `keyboard_mouse` and `mouse_sensitivity` in the title's `xcl.toml`.

| Input | Controller equivalent |
| --- | --- |
| WASD / Mouse | Left / right stick |
| Mouse buttons / wheel | Triggers / bumpers |
| Enter or Space / Escape or Backspace | A / B |
| E / Q | Y / X |
| Arrow keys | D-pad |
| Shift / Ctrl | Stick clicks |
| Tab / P | View / Menu |

## Compatibility

The following results are reported by the upstream XCL project. They are not independent validation of the current XCLayer revision.

| Title | Reported status |
| --- | --- |
| Minecraft Preview | Playable |
| Unicorn Overlord | Playable |
| Fortnite | Limited by online-service requirements |
| XDefiant | Limited by discontinued online services |
| Other retail titles | Experimental; no general gameplay guarantee |

## Features

- Native x64 execution and console PE loading, including imports, TLS and exception handling.
- High-level implementations of console kernel, runtime, storage, input and audio interfaces.
- RDNA2 shader recompilation, asynchronous pipeline compilation and persistent shader caching.
- Direct3D 12 graphics and compute translation, texture detiling and GPU memory management.
- Experimental mesh shaders, ray tracing and console upscaling support.
- BCPack decoding for BC1, BC3, BC4, BC5, BC6H and BC7 streams; BC2 and hardware swizzle options remain unsupported.
- Diagnostic tooling for memory allocation, resource lifetime, streaming and graphics errors.

## Troubleshooting

Title-specific data is stored at:

```text
%LOCALAPPDATA%\XCL\<title>\
```

| File or directory | Purpose |
| --- | --- |
| `xcl.toml` | Title configuration |
| `logs/xcl.log` | Runtime log |
| `logs/report.json` | Compatibility and crash diagnostics |
| `overlay/` | Redirected game writes |
| `saves/` | Saved game data |
| `cache/` | Cached runtime data |

If startup fails, check the DXC version, GPU capabilities, Gaming Services and available system commit. The runtime's 24 GiB commit-headroom warning is advisory, not a fixed RAM requirement.

When reporting a problem, include the XCLayer commit, Windows version, GPU, driver version, title version and relevant logs. Remove personal information and do not attach game binaries or copyrighted game assets.

## Development

Run the existing test suite from the repository root:

```powershell
.\scripts\test.ps1
```

GPU tests require compatible hardware and DXC. Some tests are skipped when their hardware or sample-data prerequisites are absent; a successful test command alone does not establish full compatibility.

- [Technical handoff](HANDOFF.md)
- [Architecture and design](docs/specs/2026-11-20-xcl-design.md)
- [Wave64 translation](docs/wave64-on-wave32.md)
- [Patch 25 engineering notes](PATCH25_AUDIT.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## License

XCLayer is distributed under the [GPL-2.0-only license](LICENSE). See [THIRD_PARTY.md](THIRD_PARTY.md) for third-party notices and attribution.

Based on the original [XCL project](https://github.com/ivall/xcl). XCLayer is an independent fork and is not affiliated with Microsoft or Xbox. No game files or proprietary console SDK binaries are included.
