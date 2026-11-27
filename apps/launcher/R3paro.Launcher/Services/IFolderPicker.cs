namespace R3paro.Launcher.Services;

public interface IFolderPicker
{
    /// <summary>Asks the player for a folder; null when cancelled.</summary>
    Task<string?> PickFolderAsync();
}
