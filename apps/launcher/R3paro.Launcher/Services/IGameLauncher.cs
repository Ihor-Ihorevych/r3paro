namespace R3paro.Launcher.Services;

/// <param name="Running">When started: completes when the game process exits.</param>
public sealed record LaunchResult(bool Started, string? Error, Task? Running = null);

public interface IGameLauncher
{
    Task<LaunchResult> LaunchAsync(string gameFolder, CancellationToken ct = default);
}
