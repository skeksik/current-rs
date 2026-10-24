using System.Diagnostics;
using Current.Bindings;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Current.Shell.Gateway;

/// <summary>
/// Signing in to a saved server's gateway again (ADR 0017), from Settings → Accounts: runs one <see cref="GatewaySignInFlow"/> through
/// the same password dialog, sign-in window and extra headers as add-server, until the core says it is done or the user cancels. The
/// Jellyfin session and its token stay as they are throughout; the core resumes the session once the gateway lets it through.
/// </summary>
internal sealed class GatewaySignInRunner(XamlRoot root, DispatcherQueueUi ui)
{
    /// <summary>What happened, for the page's notice. Null message: cancelled, nothing to say.</summary>
    internal sealed record Outcome(InfoBarSeverity Severity, string? Message);

    /// <summary>
    /// Runs the flow. <paramref name="headersFirst"/>: the user asked for the extra headers ("Advanced"), so they are offered in
    /// place of the password or the sign-in window whenever the gateway asks for anything.
    /// </summary>
    public async Task<Outcome> RunAsync(CoreHost core, ServerRef server, bool headersFirst)
    {
        using var flow = core.SignInToGateway(server);
        try
        {
            await flow.StartAsync();
            string? user = null;
            while (true)
            {
                switch (flow.State)
                {
                    case GatewaySignInState.Done done:
                        return done.Remembered
                            ? new Outcome(InfoBarSeverity.Success, headersFirst
                                ? "The gateway lets Current through. Nothing needed changing."
                                : "Signed in to the gateway. The server's session goes on.")
                            : new Outcome(
                                InfoBarSeverity.Warning,
                                "Signed in to the gateway for this session only: Windows Credential Manager isn't available.");
                    case GatewaySignInState.Failed failed:
                        return new Outcome(InfoBarSeverity.Error, ProblemText.GatewaySignIn(failed.Problem));
                    case GatewaySignInState.Unsupported:
                        return new Outcome(InfoBarSeverity.Error, ProblemText.GatewayUnsupported);
                    case GatewaySignInState.Password or GatewaySignInState.WebSignIn or GatewaySignInState.Refused
                        when headersFirst || flow.State is GatewaySignInState.Refused:
                        if (!await AskHeadersAsync(flow))
                        {
                            return Cancelled(flow);
                        }
                        break;
                    case GatewaySignInState.Password password:
                        var answer = await GatewayPasswordDialog.AskAsync(root, password.Origin, password.Realm, password.Wrong, false, user);
                        if (answer is null)
                        {
                            return Cancelled(flow);
                        }
                        user = answer.User;
                        await flow.SubmitPasswordAsync(answer.User, answer.Password);
                        break;
                    case GatewaySignInState.WebSignIn web:
                        if (!WebViewRuntime.IsInstalled())
                        {
                            return new Outcome(InfoBarSeverity.Error, WebViewRuntime.Missing);
                        }
                        if (!await SignInOnTheWebAsync(flow, web))
                        {
                            return Cancelled(flow);
                        }
                        break;
                    default:
                        // Checking: every step above awaits its probe, so this is not expected; wait for the next state.
                        await NextStateAsync(flow);
                        break;
                }
            }
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: signing in to the gateway failed: {error.GetType().Name}");
            flow.Cancel();
            return new Outcome(InfoBarSeverity.Error, ProblemText.Unexpected);
        }
    }

    private static Outcome Cancelled(GatewaySignInFlow flow)
    {
        flow.Cancel();
        return new Outcome(InfoBarSeverity.Informational, null);
    }

    /// <summary>The extra headers in a dialog; it stays open with the core's reason when it refuses them. False: cancelled.</summary>
    private async Task<bool> AskHeadersAsync(GatewaySignInFlow flow)
    {
        var editor = new ExtraHeadersEditor { ShowsUseButton = false };
        var dialog = new ContentDialog
        {
            XamlRoot = root,
            Title = "Extra headers",
            Content = new ScrollViewer { Content = editor },
            PrimaryButtonText = "Use these headers",
            CloseButtonText = "Cancel",
            DefaultButton = ContentDialogButton.Primary,
        };
        dialog.Opened += (_, _) => editor.FocusFirst();
        dialog.PrimaryButtonClick += async (_, args) =>
        {
            var deferral = args.GetDeferral();
            editor.IsEditable = false;
            try
            {
                editor.HideError();
                await flow.SubmitHeadersAsync(editor.Read());
            }
            catch (GatewayInputException error)
            {
                // Nothing was sent: the dialog stays, with the reason.
                editor.ShowError(ProblemText.GatewayInput(error.Problem));
                args.Cancel = true;
            }
            catch (FlowNotReadyException)
            {
                // The state moved on: the runner reads it next.
            }
            finally
            {
                editor.IsEditable = true;
                deferral.Complete();
            }
        };
        var result = await dialog.ShowAsync();
        editor.Clear();
        return result == ContentDialogResult.Primary;
    }

    /// <summary>
    /// The sign-in window on the address; it closes once the core moves on from the web sign-in. False: the user closed it.
    /// </summary>
    private async Task<bool> SignInOnTheWebAsync(GatewaySignInFlow flow, GatewaySignInState.WebSignIn web)
    {
        var finished = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        using var state = flow.ObserveState(ui);
        var window = new SignInWindow(web.Url, web.Origin, async cookies =>
        {
            try
            {
                await flow.SubmitCookiesAsync(cookies);
            }
            catch (FlowNotReadyException)
            {
                // The core moved on meanwhile; its state says where to.
            }
        });
        state.PropertyChanged += (_, _) =>
        {
            if (state.Value is not (GatewaySignInState.WebSignIn or GatewaySignInState.Checking))
            {
                finished.TrySetResult(true);
            }
        };
        window.Cancelled += (_, _) => finished.TrySetResult(false);
        window.Activate();
        var moved = await finished.Task;
        window.CloseFromApp();
        return moved;
    }

    private async Task NextStateAsync(GatewaySignInFlow flow)
    {
        var changed = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        using var state = flow.ObserveState(ui);
        state.PropertyChanged += (_, _) => changed.TrySetResult();
        if (flow.State is not GatewaySignInState.Checking)
        {
            return;
        }
        await changed.Task;
    }
}
