using System.Text.Json;
using System.Text.Json.Serialization;

namespace R3paro.Launcher.Services;

/// <summary>Stores <see cref="LauncherSettings"/> as JSON; a missing or unreadable file means defaults.</summary>
public sealed class JsonSettingsStore(string filePath) : ISettingsStore
{
    public static string DefaultPath =>
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "r3paro", "launcher.json");

    public LauncherSettings Load()
    {
        try
        {
            using var stream = File.OpenRead(filePath);
            return JsonSerializer.Deserialize(stream, SettingsJsonContext.Default.LauncherSettings) ?? new LauncherSettings();
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException or JsonException)
        {
            return new LauncherSettings();
        }
    }

    public void Save(LauncherSettings settings)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(filePath))!);
        File.WriteAllText(filePath, JsonSerializer.Serialize(settings, SettingsJsonContext.Default.LauncherSettings));
    }
}

[JsonSourceGenerationOptions(WriteIndented = true)]
[JsonSerializable(typeof(LauncherSettings))]
internal sealed partial class SettingsJsonContext : JsonSerializerContext;
