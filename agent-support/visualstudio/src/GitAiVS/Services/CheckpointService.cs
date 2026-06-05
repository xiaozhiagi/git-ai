using System;
using System.Diagnostics;
using System.Threading.Tasks;
using GitAiVS.Models;

namespace GitAiVS.Services
{
    /// <summary>
    /// Spawns the git-ai CLI to create checkpoints.
    /// All methods are fire-and-forget safe — they never throw.
    /// </summary>
    public sealed class CheckpointService
    {
        private readonly BinaryResolver _resolver;
        private readonly string _sessionId;

        public CheckpointService(BinaryResolver resolver)
        {
            _resolver = resolver;
            _sessionId = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds().ToString();
        }

        public string SessionId => _sessionId;

        /// <summary>
        /// Send a human (before_edit) checkpoint via agent-v1 preset.
        /// </summary>
        public Task<bool> SendBeforeEditAsync(string repoRoot, string[] willEditPaths, System.Collections.Generic.Dictionary<string, string>? dirtyFiles)
        {
            var input = new HumanInput
            {
                RepoWorkingDir = repoRoot,
                WillEditFilepaths = new System.Collections.Generic.List<string>(willEditPaths),
                DirtyFiles = dirtyFiles,
            };

            return RunCheckpointAsync(
                new[] { "checkpoint", "agent-v1", "--hook-input", "stdin" },
                input.ToJson(),
                repoRoot);
        }

        /// <summary>
        /// Send an AI agent (after_edit) checkpoint via agent-v1 preset.
        /// </summary>
        public Task<bool> SendAfterEditAsync(string repoRoot, string[] editedPaths, string agentName, System.Collections.Generic.Dictionary<string, string>? dirtyFiles)
        {
            var input = new AiAgentInput
            {
                RepoWorkingDir = repoRoot,
                EditedFilepaths = new System.Collections.Generic.List<string>(editedPaths),
                AgentName = agentName,
                Model = "unknown",
                ConversationId = _sessionId,
                DirtyFiles = dirtyFiles,
            };

            return RunCheckpointAsync(
                new[] { "checkpoint", "agent-v1", "--hook-input", "stdin" },
                input.ToJson(),
                repoRoot);
        }

        /// <summary>
        /// Send a known_human checkpoint.
        /// </summary>
        public Task<bool> SendKnownHumanAsync(string repoRoot, string editorVersion, string extensionVersion,
            System.Collections.Generic.List<string> editedPaths, System.Collections.Generic.Dictionary<string, string> dirtyFiles)
        {
            var input = new KnownHumanInput
            {
                Editor = "visualstudio",
                EditorVersion = editorVersion,
                ExtensionVersion = extensionVersion,
                Cwd = repoRoot,
                EditedFilepaths = editedPaths,
                DirtyFiles = dirtyFiles,
            };

            return RunCheckpointAsync(
                new[] { "checkpoint", "known_human", "--hook-input", "stdin" },
                input.ToJson(),
                repoRoot);
        }

        private async Task<bool> RunCheckpointAsync(string[] args, string stdinJson, string cwd)
        {
            var binaryPath = _resolver.Resolve();
            if (binaryPath == null)
                return false;

            try
            {
                var psi = new ProcessStartInfo
                {
                    FileName = binaryPath,
                    Arguments = string.Join(" ", args),
                    WorkingDirectory = cwd,
                    UseShellExecute = false,
                    RedirectStandardInput = true,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    CreateNoWindow = true,
                };

                using var proc = Process.Start(psi);
                if (proc == null) return false;

                await proc.StandardInput.WriteAsync(stdinJson);
                proc.StandardInput.Close();

                var completed = proc.WaitForExit(30_000);
                if (!completed)
                {
                    proc.Kill();
                    return false;
                }

                return proc.ExitCode == 0;
            }
            catch (Exception ex)
            {
                System.Diagnostics.Debug.WriteLine($"[git-ai] Checkpoint error: {ex.Message}");
                return false;
            }
        }
    }
}
