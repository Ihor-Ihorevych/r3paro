namespace R3paro.Launcher.Services;

public sealed record LaunchResult(bool Started, string? Error);

public interface IGameLauncher
{
    Task<LaunchResult> LaunchAsync(string gameFolder, CancellationToken ct = default);
}
