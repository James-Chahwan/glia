using System.Diagnostics;

public class Runner
{
    public void Reconcile() => Process.Start("billing", "reconcile")?.WaitForExit();
}
