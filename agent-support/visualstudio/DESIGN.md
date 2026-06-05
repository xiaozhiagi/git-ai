# git-ai Visual Studio Extension: Technical Design

## 1. Overview

This document describes the design of a Visual Studio (VSIX) extension that detects AI-generated code edits (primarily GitHub Copilot) and records them via the `git-ai` CLI. The extension follows the same architectural patterns as the existing IntelliJ plugin.

### Goals

- Detect when GitHub Copilot (inline completions or chat edits) modifies code in Visual Studio
- Record AI-authored edits by calling `git ai checkpoint agent-v1 --hook-input stdin`
- Record human edits as `known_human` checkpoints so git-ai can distinguish the before/after boundary
- Auto-install via `git ai install-hooks`

### Non-goals

- Supporting Visual Studio for Mac (discontinued by Microsoft)
- Supporting Visual Studio versions older than 2022 (17.0)

---

## 2. Background: How the existing extensions work

### 2.1 IntelliJ plugin (our primary reference)

The IntelliJ plugin uses **stack trace analysis** to detect AI edits. Every text change in IntelliJ fires a `DocumentEvent` on the EDT (Event Dispatch Thread). The plugin captures `Thread.currentThread().stackTrace` and inspects it for known AI agent class prefixes:

```
com.github.copilot.*         -> "github-copilot-jetbrains"
com.intellij.ml.llm.matterhorn.* -> "junie"
```

If a HIGH-confidence match is found, the edit is recorded as an AI checkpoint. The plugin has three listeners:

| Listener | Trigger | Checkpoint type |
|---|---|---|
| `DocumentChangeListener` | `ITextBuffer.Changed` equivalent | `agent-v1` (AI) with before/after pair |
| `VfsRefreshListener` | Disk writes from external processes | `agent-v1` (AI) sweep checkpoint |
| `DocumentSaveListener` | User-initiated saves | `known_human` |

### 2.2 VS Code extension

The VS Code extension uses a completely different strategy: **URI scheme sniffing**. When Copilot's chat feature edits a file, VS Code opens a temporary document with the URI scheme `chat-editing-snapshot-text-model://`. The extension watches for these URIs to detect AI edits.

This mechanism is VS Code-specific and does not exist in Visual Studio or IntelliJ.

### 2.3 Why Visual Studio is closer to IntelliJ

| Capability | VS Code | IntelliJ | Visual Studio |
|---|---|---|---|
| Text change events | `onDidChangeTextDocument` | `BulkAwareDocumentListener` | `ITextBuffer.Changed` |
| Thread model | Multi-process (extension host) | Single EDT | Single UI thread |
| Stack trace visibility | Not useful (cross-process) | Full call chain visible | Full call chain visible |
| AI edit URI tagging | Yes (`chat-editing-snapshot-text-model://`) | No | No |
| Extension language | TypeScript | Kotlin/JVM | C#/.NET |

Because Visual Studio runs Copilot extensions in-process on the UI thread (like IntelliJ), stack trace analysis is the correct detection strategy.

---

## 3. Architecture

### 3.1 Component diagram

```
┌─────────────────────────────────────────────────────────┐
│                    Visual Studio                         │
│                                                         │
│  ┌──────────────────────┐                               │
│  │   GitAiPackage       │  (AsyncPackage entry point)   │
│  │   ├── BinaryResolver │  Locate git-ai binary         │
│  │   └── Registers:     │                               │
│  │       ├── TextBufferListener   ──┐                   │
│  │       ├── TabCompletionFilter  ──┤                   │
│  │       └── DocumentSaveListener ──┤                   │
│  └──────────────────────┘           │                   │
│                                     ▼                   │
│  ┌──────────────────────────────────────────────┐       │
│  │  CopilotEditDetector                         │       │
│  │  Inspects Environment.StackTrace for:        │       │
│  │   • GitHub.Copilot.*                         │       │
│  │   • Microsoft.VisualStudio.Copilot.*         │       │
│  └──────────────────────────────────────────────┘       │
│                    │                                     │
│                    ▼                                     │
│  ┌──────────────────────────────────────────────┐       │
│  │  CheckpointService                           │       │
│  │  Spawns: git-ai checkpoint agent-v1          │       │
│  │          --hook-input stdin                   │       │
│  │  Writes JSON to stdin, reads exit code       │       │
│  └──────────────────────────────────────────────┘       │
│                    │                                     │
└────────────────────│─────────────────────────────────────┘
                     ▼
              ┌─────────────┐
              │  git-ai CLI │
              │  (Rust)     │
              └──────┬──────┘
                     ▼
              ┌─────────────┐
              │ Git Notes   │
              │ refs/notes/ │
              │    ai       │
              └─────────────┘
```

### 3.2 Event flow

```
User accepts Copilot suggestion (Tab) or Copilot chat applies edit
    │
    ▼
ITextBuffer.Changed fires on UI thread
    │
    ▼
TextBufferListener captures Environment.StackTrace
    │
    ▼
CopilotEditDetector.Analyze(stackTrace)
    │
    ├── HIGH confidence match (Copilot assembly found)
    │   │
    │   ├── 1. Send "human" before_edit checkpoint (pre-edit content)
    │   │      { "type": "human", "repo_working_dir": "...", "will_edit_filepaths": [...], "dirty_files": {...} }
    │   │
    │   └── 2. Debounce 300ms, then send "ai_agent" after_edit checkpoint
    │          { "type": "ai_agent", "repo_working_dir": "...", "edited_filepaths": [...],
    │            "agent_name": "github-copilot-visualstudio", "model": "unknown",
    │            "conversation_id": "<session_id>", "dirty_files": {...} }
    │
    └── No match (human edit)
        │
        └── On save: send known_human checkpoint (debounced 500ms)
            { "editor": "visualstudio", "editor_version": "17.x", "extension_version": "0.1.0",
              "cwd": "...", "edited_filepaths": [...], "dirty_files": {...} }
```

---

## 4. Detailed component design

### 4.1 GitAiPackage (entry point)

**File**: `src/GitAiVS/GitAiPackage.cs`

The `AsyncPackage` subclass that Visual Studio loads on startup. Responsibilities:

- Resolve the git-ai binary path (via `BinaryResolver`)
- Register `IVsTextViewCreationListener` (to attach `TextBufferListener` to every editor)
- Subscribe to `IVsRunningDocTableEvents` (for `DocumentSaveListener`)
- Display info bar if git-ai is not installed

```csharp
[PackageRegistration(UseManagedResourcesOnly = true, AllowsBackgroundLoading = true)]
[ProvideAutoLoad(VSConstants.UICONTEXT.SolutionExists_string, PackageAutoLoadFlags.BackgroundLoad)]
[Guid(PackageGuidString)]
public sealed class GitAiPackage : AsyncPackage
{
    protected override async Task InitializeAsync(CancellationToken cancellationToken, IProgress<ServiceProgressData> progress)
    {
        await JoinableTaskFactory.SwitchToMainThreadAsync(cancellationToken);
        // 1. Resolve binary
        // 2. Register listeners
        // 3. Show info bar if binary not found
    }
}
```

**Auto-load context**: `SolutionExists` -- the package loads when any solution is opened, which is the earliest useful point (files are available to edit).

### 4.2 BinaryResolver

**File**: `src/GitAiVS/Services/BinaryResolver.cs`

Locates the `git-ai` (or `git-ai.exe`) binary. Search order:

1. `%USERPROFILE%\.git-ai\bin\git-ai.exe` (standard install location on Windows)
2. `%USERPROFILE%\.git-ai-local-dev\gitwrap\bin\git-ai.exe` (nix dev path)
3. `PATH` lookup via `where git-ai` (Windows) or `which git-ai` (Mac/Linux)

After finding the binary, runs `git-ai version` to verify it meets the minimum version requirement (currently `1.0.23`, matching IntelliJ). Caches the resolved path for the session.

```csharp
public class BinaryResolver
{
    private static readonly Version MinVersion = new(1, 0, 23);
    private string _cachedPath;

    public string Resolve()
    {
        if (_cachedPath != null && File.Exists(_cachedPath)) return _cachedPath;

        // Check known paths, then PATH
        // Verify version
        // Cache and return
    }
}
```

### 4.3 GitRepoResolver

**File**: `src/GitAiVS/Services/GitRepoResolver.cs`

Finds the git repository root for a given file path by walking up the directory tree looking for a `.git` directory. This is the `repo_working_dir` value sent in every checkpoint.

```csharp
public static class GitRepoResolver
{
    public static string FindRepoRoot(string filePath)
    {
        var dir = Path.GetDirectoryName(filePath);
        while (dir != null)
        {
            if (Directory.Exists(Path.Combine(dir, ".git")))
                return dir;
            dir = Path.GetDirectoryName(dir);
        }
        return null;
    }
}
```

### 4.4 CopilotEditDetector (stack trace analysis)

**File**: `src/GitAiVS/Detection/CopilotEditDetector.cs`

The core detection logic, directly modeled after IntelliJ's `StackTraceAnalyzer.kt`.

**How it works**: When `ITextBuffer.Changed` fires, the calling thread's stack trace contains frames from whatever code triggered the change. If Copilot triggered it, frames from Copilot's assemblies will be present.

**Known agent patterns**:

| Agent name | Package/namespace prefixes (HIGH confidence) | Class name patterns (MEDIUM confidence) |
|---|---|---|
| `github-copilot-visualstudio` | `GitHub.Copilot`, `Microsoft.VisualStudio.Copilot` | `copilot` |

**Confidence levels**:

- **HIGH**: A stack frame's full class name starts with a known package prefix (e.g., `GitHub.Copilot.InlineCompletion.Handler`). Only HIGH-confidence matches trigger checkpoints.
- **MEDIUM**: A stack frame's class name contains a known keyword (e.g., class name contains `copilot`). Logged for debugging but does not trigger checkpoints.
- **NONE**: No AI agent patterns detected. The edit is treated as human.

```csharp
public enum Confidence { None, Medium, High }

public record AnalysisResult(string AgentName, Confidence Confidence, List<StackFrame> RelevantFrames);

public static class CopilotEditDetector
{
    private static readonly AgentPattern[] KnownAgents = new[]
    {
        new AgentPattern(
            Name: "github-copilot-visualstudio",
            PackagePrefixes: new[] { "GitHub.Copilot", "Microsoft.VisualStudio.Copilot" },
            ClassKeywords: new[] { "copilot" }
        ),
    };

    public static AnalysisResult Analyze(StackTrace stackTrace)
    {
        // Walk frames, match against patterns
        // Return first HIGH-confidence match, or NONE
    }
}
```

**Important**: The exact namespace prefixes for GitHub Copilot in Visual Studio will need to be discovered empirically by installing Copilot in VS, making edits, and logging the stack traces. The prefixes listed above are our best starting guess based on the extension's known publisher (`GitHub.Copilot`). We will need a discovery phase where we log all stack frames on every text change to identify the exact patterns.

### 4.5 TabCompletionFilter (inline completion detection)

**File**: `src/GitAiVS/Detection/TabCompletionFilter.cs`

Detects when a user accepts an inline Copilot suggestion by pressing Tab.

**Strategy**: Implement `IOleCommandTarget` and attach it as a command filter to the text view. When `VSStd2KCmdID.TAB` is executed and an inline completion adornment is visible, set a short-lived flag (`_tabAcceptedAt`) that `TextBufferListener` checks on the next `ITextBuffer.Changed` event.

```csharp
public class TabCompletionFilter : IOleCommandTarget
{
    private DateTime _tabAcceptedAt = DateTime.MinValue;
    private static readonly TimeSpan TabWindow = TimeSpan.FromMilliseconds(200);

    public bool WasRecentTabAccept => (DateTime.UtcNow - _tabAcceptedAt) < TabWindow;

    public int Exec(ref Guid pguidCmdGroup, uint nCmdID, uint nCmdexecopt, IntPtr pvaIn, IntPtr pvaOut)
    {
        if (pguidCmdGroup == VSConstants.VSStd2K && nCmdID == (uint)VSStd2KCmdID.TAB)
        {
            // Check if inline completion is showing
            // If so, set _tabAcceptedAt = DateTime.UtcNow
        }
        return _nextTarget.Exec(ref pguidCmdGroup, nCmdID, nCmdexecopt, pvaIn, pvaOut);
    }
}
```

**Detecting whether an inline completion is visible**: This is the trickiest part. Options:
1. Check for a Copilot-specific adornment layer on the text view
2. Check if the `ICompletionSession` (or equivalent) is active
3. Use the Tab key heuristic: if the stack trace in the subsequent `Changed` event contains Copilot frames, it was an accepted suggestion

We will use option 3 as the primary approach (stack trace on the `Changed` event is sufficient), with the `TabCompletionFilter` as a supplementary signal.

### 4.6 TextBufferListener

**File**: `src/GitAiVS/Listeners/TextBufferListener.cs`

Attaches to every opened text editor and listens for `ITextBuffer.Changed` events.

**Lifecycle**: Implements `IVsTextViewCreationListener` (with `[Export]` MEF attribute). Visual Studio calls `VsTextViewCreated` for every editor that opens. We attach our handler to the `ITextBuffer` associated with that view.

**On each change event**:

1. Capture `Environment.StackTrace` (or `new StackTrace()`)
2. Pass to `CopilotEditDetector.Analyze()`
3. If HIGH confidence AI edit:
   a. If no recent `before_edit` was sent for this file, send a human `before_edit` checkpoint with the pre-change content
   b. Cancel any pending debounce timer for this file
   c. Schedule a new 300ms debounce timer; when it fires, send an `ai_agent` `after_edit` checkpoint
4. Track the file in `_agentTouchedFiles` (same pattern as IntelliJ's `TrackedAgent`)

**Debouncing**: AI edits often arrive as rapid sequences of small changes (e.g., Copilot typing character by character). The 300ms debounce window ensures we send a single checkpoint after the burst completes, with the final file content.

```csharp
[Export(typeof(IVsTextViewCreationListener))]
[ContentType("text")]
[TextViewRole(PredefinedTextViewRoles.Editable)]
public class TextBufferListener : IVsTextViewCreationListener
{
    private readonly ConcurrentDictionary<string, TrackedAgent> _agentTouchedFiles = new();
    private readonly ConcurrentDictionary<string, CancellationTokenSource> _pendingCheckpoints = new();
    private readonly ConcurrentDictionary<string, string> _fileContentBeforeEdit = new();

    public void VsTextViewCreated(IVsTextView textViewAdapter)
    {
        // Get ITextBuffer from adapter
        // Subscribe to Changed event
        // Attach TabCompletionFilter as command filter
    }

    private void OnBufferChanged(object sender, TextContentChangedEventArgs e)
    {
        var stackTrace = new StackTrace();
        var analysis = CopilotEditDetector.Analyze(stackTrace);

        if (analysis.Confidence == Confidence.High)
        {
            HandleAiEdit(filePath, analysis, e);
        }
    }
}
```

### 4.7 DocumentSaveListener

**File**: `src/GitAiVS/Listeners/DocumentSaveListener.cs`

Listens for file save events and sends `known_human` checkpoints. Modeled after IntelliJ's `DocumentSaveListener.kt`.

**Implementation**: Subscribe to `IVsRunningDocTableEvents.OnAfterSave` via the Running Document Table (RDT). On save, debounce for 500ms per workspace root, then batch all saved files into a single `known_human` checkpoint.

**Filtering**: Skip files inside `.vs/` (Visual Studio's internal directory, equivalent to IntelliJ's `.idea/`).

```csharp
public class DocumentSaveListener : IVsRunningDocTableEvents
{
    private readonly ConcurrentDictionary<string, Timer> _pendingCheckpoints = new();
    private readonly ConcurrentDictionary<string, HashSet<string>> _pendingPaths = new();

    public int OnAfterSave(uint docCookie)
    {
        // Get file path from docCookie
        // Skip .vs/ internal paths
        // Find workspace root
        // Add to pending paths, schedule debounced checkpoint
    }
}
```

### 4.8 CheckpointService

**File**: `src/GitAiVS/Services/CheckpointService.cs`

Spawns the `git-ai` CLI process and writes JSON to stdin.

```csharp
public class CheckpointService
{
    private readonly string _binaryPath;

    // For AI edits (agent-v1 preset)
    public async Task<bool> SendAgentV1Checkpoint(AgentV1Input input, string workingDirectory)
    {
        var json = JsonSerializer.Serialize(input);
        return await RunCheckpoint(new[] { "checkpoint", "agent-v1", "--hook-input", "stdin" }, json, workingDirectory);
    }

    // For human edits (known_human preset)
    public async Task<bool> SendKnownHumanCheckpoint(KnownHumanInput input, string workingDirectory)
    {
        var json = JsonSerializer.Serialize(input);
        return await RunCheckpoint(new[] { "checkpoint", "known_human", "--hook-input", "stdin" }, json, workingDirectory);
    }

    private async Task<bool> RunCheckpoint(string[] args, string stdinJson, string cwd)
    {
        var process = new Process
        {
            StartInfo = new ProcessStartInfo
            {
                FileName = _binaryPath,
                Arguments = string.Join(" ", args),
                WorkingDirectory = cwd,
                UseShellExecute = false,
                RedirectStandardInput = true,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true,
            }
        };

        process.Start();
        await process.StandardInput.WriteAsync(stdinJson);
        process.StandardInput.Close();

        var completed = process.WaitForExit(30_000); // 30s timeout
        if (!completed)
        {
            process.Kill();
            return false;
        }

        return process.ExitCode == 0;
    }
}
```

### 4.9 JSON models (checkpoint input schemas)

**File**: `src/GitAiVS/Models/CheckpointInput.cs`

These match the Rust `AgentV1Payload` enum (from `agent_v1.rs`) and `KnownHumanPreset` exactly.

#### agent-v1: Human (before_edit)

```json
{
  "type": "human",
  "repo_working_dir": "/path/to/repo",
  "will_edit_filepaths": ["src/Program.cs"],
  "dirty_files": {
    "src/Program.cs": "file content before AI edit"
  }
}
```

#### agent-v1: AiAgent (after_edit)

```json
{
  "type": "ai_agent",
  "repo_working_dir": "/path/to/repo",
  "edited_filepaths": ["src/Program.cs"],
  "agent_name": "github-copilot-visualstudio",
  "model": "unknown",
  "conversation_id": "1717000000000",
  "dirty_files": {
    "src/Program.cs": "file content after AI edit"
  }
}
```

#### known_human

```json
{
  "editor": "visualstudio",
  "editor_version": "17.10.0",
  "extension_version": "0.1.0",
  "cwd": "/path/to/repo",
  "edited_filepaths": ["src/Program.cs"],
  "dirty_files": {
    "src/Program.cs": "file content at save time"
  }
}
```

---

## 5. CLI installer (Rust side)

### 5.1 VisualStudioInstaller

**File**: `src/mdm/agents/visual_studio.rs`

A new `HookInstaller` implementation that auto-detects Visual Studio installations and installs the VSIX extension.

**Detection**: Use `vswhere.exe` (ships with Visual Studio at `%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe`) to enumerate installed VS instances:

```
vswhere.exe -all -format json -property installationPath,installationVersion
```

**Extension install**: Use `VSIXInstaller.exe` (found at `<VS install path>\Common7\IDE\VSIXInstaller.exe`):

```
VSIXInstaller.exe /quiet /admin GitAiVS.vsix
```

**Check if installed**: Look for the extension in VS's private extension directory:
```
%LOCALAPPDATA%\Microsoft\VisualStudio\17.0_<instance>\Extensions\
```

**Registration**: Add to `get_all_installers()` in `src/mdm/agents/mod.rs`.

### 5.2 Platform scope

The `VisualStudioInstaller` is **Windows-only**. Visual Studio for Mac has been discontinued. The installer should return `tool_installed: false` on non-Windows platforms.

---

## 6. Project structure

```
agent-support/visualstudio/
├── DESIGN.md                          # This document
├── README.md                          # Build/debug/test instructions
├── GitAiVS.sln                        # Solution file
└── src/
    └── GitAiVS/
        ├── GitAiVS.csproj             # Project file (targets net48, VS 2022+)
        ├── source.extension.vsixmanifest
        ├── GitAiPackage.cs
        ├── Services/
        │   ├── BinaryResolver.cs
        │   ├── GitRepoResolver.cs
        │   └── CheckpointService.cs
        ├── Detection/
        │   ├── CopilotEditDetector.cs
        │   └── TabCompletionFilter.cs
        ├── Listeners/
        │   ├── TextBufferListener.cs
        │   └── DocumentSaveListener.cs
        └── Models/
            └── CheckpointInput.cs
```

---

## 7. Open questions and risks

### 7.1 Stack trace pattern discovery

The exact namespace prefixes for GitHub Copilot's Visual Studio extension are not publicly documented. We need an empirical discovery phase:

1. Install the extension with verbose stack trace logging enabled
2. Accept Copilot inline suggestions and chat edits
3. Capture and catalog the stack frame class names
4. Update `CopilotEditDetector.KnownAgents` with the discovered patterns

**Mitigation**: The extension includes a debug logging mode (similar to IntelliJ's `logDocumentChange`) that dumps full stack traces to the VS output window. This allows pattern discovery without rebuilding.

### 7.2 Thread model assumptions

We assume Copilot edits fire `ITextBuffer.Changed` on the UI thread with a visible stack trace. If Copilot uses `ITextBuffer.CreateEdit()` from a background thread and marshals to the UI thread via `Dispatcher.Invoke`, the Copilot frames may be lost. In that case, we would fall back to the `TabCompletionFilter` heuristic for inline completions and would need an alternative approach for chat edits.

**Mitigation**: If stack trace analysis proves unreliable, we can explore:
- Monitoring Copilot's `ITextBufferEdit` tags/properties
- Watching for Copilot-specific `ITextVersion` metadata
- Using DTE automation events (`TextDocumentKeyPressEvents`)

### 7.3 VS Marketplace publishing

Publishing a VSIX to the Visual Studio Marketplace requires:
- A Visual Studio Marketplace publisher account
- Code signing certificate (recommended but not required)
- Extension icon and metadata

This is a one-time setup cost handled outside the PR.

### 7.4 Performance

Stack trace capture (`new StackTrace()`) is not free. On every keystroke, we capture a stack trace. In IntelliJ, this has not been a measurable performance issue because:
- Stack traces are shallow (typically < 50 frames)
- The JVM has optimized `Thread.getStackTrace()`

In .NET, `Environment.StackTrace` (or `new StackTrace()`) is similarly lightweight. However, we should benchmark this in Visual Studio with large solutions to confirm there is no perceptible typing lag.

**Mitigation**: If performance is an issue, we can:
- Only capture stack traces when `ITextBuffer.Changed` has characteristics suggesting non-human input (e.g., large insertions, multi-line changes)
- Sample stack traces (e.g., only every 3rd change event)

---

## 8. Testing strategy

### 8.1 Unit tests

- `CopilotEditDetector`: Test with synthetic stack traces containing various combinations of Copilot and non-Copilot frames. Verify confidence levels and agent name extraction.
- `BinaryResolver`: Test search order with mocked file system paths.
- `CheckpointInput` serialization: Verify JSON output matches the expected schemas.
- `GitRepoResolver`: Test with nested directories, repos with worktrees, and non-repo paths.

### 8.2 Integration tests

- Install the extension in VS 2022 with Copilot enabled
- Accept inline suggestions and verify `agent-v1` checkpoints are created
- Use Copilot chat to edit files and verify checkpoints
- Type manually and verify `known_human` checkpoints on save
- Run `git ai status` to confirm attribution is recorded
- Run `git ai log` after committing to verify notes are attached

### 8.3 Manual QA checklist

- [ ] Extension activates on solution open
- [ ] Info bar shown if git-ai is not installed
- [ ] No typing lag with extension enabled in a large solution
- [ ] Correct checkpoint preset (`agent-v1`) used for all events
- [ ] Debouncing works (rapid edits produce single checkpoint)
- [ ] Extension handles git-ai binary not on PATH gracefully
- [ ] Extension handles git-ai process timeout gracefully

