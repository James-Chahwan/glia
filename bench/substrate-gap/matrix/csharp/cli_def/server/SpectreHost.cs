using Spectre.Console.Cli;

namespace Billing;

public static class SpectreHost
{
    public static int Run(string[] args)
    {
        var app = new CommandApp();
        app.Configure(c => c.AddCommand<SyncCommand>("sync"));
        return app.Run(args);
    }
}

public class SyncCommand : Command
{
    public override int Execute(CommandContext ctx) => 0;
}
