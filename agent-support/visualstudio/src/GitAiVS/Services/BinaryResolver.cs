using System;
using System.Diagnostics;
using System.IO;

namespace GitAiVS.Services
{
    /// <summary>
    /// Locates the git-ai binary on the system.
    /// Search order:
    ///   1. %USERPROFILE%\.git-ai\bin\git-ai.exe  (production install)
    ///   2. %USERPROFILE%\.git-ai-local-dev\gitwrap\bin\git-ai.exe  (nix dev)
    ///   3. PATH lookup via "where git-ai"
    /// </summary>
    public sealed class BinaryResolver
    {
        private static readonly Version MinVersion = new(1, 0, 23);
        private string? _cachedPath;
        private Version? _cachedVersion;

        public string? ResolvedPath => _cachedPath;
        public Version? ResolvedVersion => _cachedVersion;

        public string? Resolve()
        {
            if (_cachedPath != null && File.Exists(_cachedPath))
                return _cachedPath;

            _cachedPath = null;
            _cachedVersion = null;

            var path = FindBinary();
            if (path == null)
                return null;

            var version = GetVersion(path);
            if (version == null || version < MinVersion)
                return null;

            _cachedPath = path;
            _cachedVersion = version;
            return path;
        }

        public void Reset()
        {
            _cachedPath = null;
            _cachedVersion = null;
        }

        private static string? FindBinary()
        {
            var home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
            var isWindows = Environment.OSVersion.Platform == PlatformID.Win32NT;
            var ext = isWindows ? ".exe" : "";

            string[] knownPaths = isWindows
                ? new[]
                {
                    Path.Combine(home, ".git-ai", "bin", "git-ai.exe"),
                    Path.Combine(home, ".git-ai-local-dev", "gitwrap", "bin", "git-ai.exe"),
                }
                : new[]
                {
                    Path.Combine(home, ".git-ai", "bin", "git-ai"),
                    Path.Combine(home, ".git-ai-local-dev", "gitwrap", "bin", "git-ai"),
                };

            foreach (var candidate in knownPaths)
            {
                if (File.Exists(candidate))
                    return candidate;
            }

            return TryPathLookup(isWindows);
        }

        private static string? TryPathLookup(bool isWindows)
        {
            try
            {
                var psi = new ProcessStartInfo
                {
                    FileName = isWindows ? "cmd" : "/bin/sh",
                    Arguments = isWindows ? "/c where git-ai" : "-l -c \"which git-ai\"",
                    UseShellExecute = false,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    CreateNoWindow = true,
                };

                using var proc = Process.Start(psi);
                if (proc == null) return null;

                var output = proc.StandardOutput.ReadToEnd().Trim();
                proc.WaitForExit(5000);
                if (!proc.HasExited) { proc.Kill(); return null; }

                if (proc.ExitCode != 0) return null;

                var firstLine = output.Split(new[] { '\r', '\n' }, StringSplitOptions.RemoveEmptyEntries);
                if (firstLine.Length > 0 && File.Exists(firstLine[0]))
                    return firstLine[0];
            }
            catch
            {
                // PATH lookup is best-effort
            }

            return null;
        }

        private static Version? GetVersion(string binaryPath)
        {
            try
            {
                var psi = new ProcessStartInfo
                {
                    FileName = binaryPath,
                    Arguments = "version",
                    UseShellExecute = false,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    CreateNoWindow = true,
                };

                using var proc = Process.Start(psi);
                if (proc == null) return null;

                var output = proc.StandardOutput.ReadToEnd().Trim();
                proc.WaitForExit(5000);
                if (!proc.HasExited) { proc.Kill(); return null; }
                if (proc.ExitCode != 0) return null;

                return ParseVersion(output);
            }
            catch
            {
                return null;
            }
        }

        internal static Version? ParseVersion(string versionString)
        {
            // Expected: "1.0.39" or "1.0.39 (debug)"
            var part = versionString.Split(' ')[0];
            var segments = part.Split('.');
            if (segments.Length < 3) return null;

            if (int.TryParse(segments[0], out var major)
                && int.TryParse(segments[1], out var minor)
                && int.TryParse(segments[2].Split('-', '+')[0], out var patch))
            {
                return new Version(major, minor, patch);
            }

            return null;
        }
    }
}
