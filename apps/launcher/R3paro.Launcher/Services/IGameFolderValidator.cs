namespace R3paro.Launcher.Services;

public sealed record ValidationResult(bool IsValid, string Message);

public interface IGameFolderValidator
{
    ValidationResult Validate(string? folder);
}
