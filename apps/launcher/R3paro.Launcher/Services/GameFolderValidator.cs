namespace R3paro.Launcher.Services;

/// <summary>Checks that a folder is a usable HP3 install: System/Core.u, Maps, and a writable Save folder.
/// Names are matched case-insensitively because Linux/macOS copies are often lowercase.</summary>
public sealed class GameFolderValidator : IGameFolderValidator
{
    private const string NotGameFolder = "this is not the HP3 game folder.";

    public ValidationResult Validate(string? folder)
    {
        if (string.IsNullOrWhiteSpace(folder))
            return Fail("Choose the game folder.");
        if (!Directory.Exists(folder))
            return Fail("Folder does not exist.");

        var system = FindChild(folder, "System", directory: true);
        if (system is null)
            return Fail($"No System folder: {NotGameFolder}");
        if (FindChild(system, "Core.u", directory: false) is null)
            return Fail($"{Path.Combine("System", "Core.u")} is missing.");
        if (FindChild(folder, "Maps", directory: true) is null)
            return Fail($"No Maps folder: {NotGameFolder}");

        if (!IsWritable(folder))
            return Fail("The folder is not writable (saves go to Save).");

        return new ValidationResult(true, "Ready.");
    }

    private static ValidationResult Fail(string message) => new(false, message);

    internal static string? FindChild(string parent, string name, bool directory)
    {
        try
        {
            var entries = directory ? Directory.EnumerateDirectories(parent) : Directory.EnumerateFiles(parent);
            return entries.FirstOrDefault(e => string.Equals(Path.GetFileName(e), name, StringComparison.OrdinalIgnoreCase));
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            return null;
        }
    }

    private static bool IsWritable(string folder)
    {
        try
        {
            var save = FindChild(folder, "Save", directory: true) ?? Directory.CreateDirectory(Path.Combine(folder, "Save")).FullName;
            var probe = Path.Combine(save, ".r3paro-write-test");
            File.WriteAllText(probe, "");
            File.Delete(probe);
            return true;
        }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException)
        {
            return false;
        }
    }
}
