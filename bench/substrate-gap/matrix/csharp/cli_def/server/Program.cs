using System.CommandLine;

var root = new RootCommand("billing tool");
var sync = new Command("reconcile", "Reconcile invoices");
root.AddCommand(sync);
return await root.InvokeAsync(args);
