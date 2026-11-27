namespace R3paro.Launcher.Services;

public sealed record LauncherSettings(string? GameFolder = null, string? EnginePath = null);

public interface ISettingsStore
{
    LauncherSettings Load();
    void Save(LauncherSettings settings);
}
