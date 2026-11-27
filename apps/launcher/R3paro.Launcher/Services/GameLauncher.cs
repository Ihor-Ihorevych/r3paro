using System.ComponentModel;
using System.Diagnostics;

namespace R3paro.Launcher.Services;

/// <summary>Starts hp3 with the game folder as its only argument. A non-zero exit within
/// <c>earlyExitWindow</c> is reported as a failure carrying the tail of stderr.</summary>
public sealed class GameLauncher(ISettingsStore settings, string baseDir, TimeSpan earlyExitWindow) : IGameLauncher
{
    private const int TailLines = 20;
    private const int KeptLines = 200;

    public static string ResolveEnginePath(string baseDir, string? configured, bool isWindows) =>
        !string.IsNullOrWhiteSpace(configured) ? configured : Path.Combine(baseDir, isWindows ? "hp3.exe" : "hp3");

    public static string Tail(string text, int lines)
    {
        var all = text.Replace("\r\n", "\n").TrimEnd('\n').Split('\n');
        return string.Join("\n", all.Skip(Math.Max(0, all.Length - lines)));
    }

    public async Task<LaunchResult> LaunchAsync(string gameFolder, CancellationToken ct = default)
    {
        var engine = ResolveEnginePath(baseDir, settings.Load().EnginePath, OperatingSystem.IsWindows());
        if (!File.Exists(engine))
            return new LaunchResult(false, $"Engine not found: {engine}");

        var psi = new ProcessStartInfo(engine)
        {
            WorkingDirectory = Path.GetDirectoryName(Path.GetFullPath(engine))!,
            UseShellExecute = false,
            RedirectStandardError = true,
        };
        psi.ArgumentList.Add(gameFolder);

        // Bounded buffer: the game may run for hours and keep writing to stderr, which we must keep draining.
        var stderr = new Queue<string>();
        var process = new Process { StartInfo = psi };
        process.ErrorDataReceived += (_, e) =>
        {
            if (e.Data is null) return;
            lock (stderr)
            {
                stderr.Enqueue(e.Data);
                if (stderr.Count > KeptLines) stderr.Dequeue();
            }
        };

        try
        {
            process.Start();
        }
        catch (Win32Exception e)
        {
            process.Dispose();
            return new LaunchResult(false, $"Could not start {engine}: {e.Message}");
        }
        process.BeginErrorReadLine();

        var exited = process.WaitForExitAsync(CancellationToken.None);
        if (await Task.WhenAny(exited, Task.Delay(earlyExitWindow, ct)) != exited)
        {
            // The game is running; keep draining stderr and release the handle when it quits.
            // The launcher must outlive the game: hp3 panics on a stderr write once the pipe's reader is gone.
            var running = exited.ContinueWith(_ => process.Dispose(), TaskScheduler.Default);
            return new LaunchResult(true, null, running);
        }

        // WaitForExitAsync has already waited for redirected stderr to reach EOF.
        var code = process.ExitCode;
        process.Dispose();
        if (code == 0)
            return new LaunchResult(true, null, Task.CompletedTask);

        string text;
        lock (stderr) text = string.Join("\n", stderr);
        return new LaunchResult(false, text.Length > 0 ? Tail(text, TailLines) : $"The game exited with code {code}.");
    }
}
