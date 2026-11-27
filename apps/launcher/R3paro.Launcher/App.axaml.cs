using Avalonia;
using Avalonia.Markup.Xaml;

namespace R3paro.Launcher;

public partial class App : Application
{
    public override void Initialize() => AvaloniaXamlLoader.Load(this);
}
