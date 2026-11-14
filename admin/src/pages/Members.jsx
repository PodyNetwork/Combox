// Who can manage the company. The owner invites people by email; they join
// when they sign up with that email and turn on two-factor authentication.
import { useCallback, useEffect, useState } from "react";
import { platform } from "../api.js";
import { Badge, Button, ErrorNote, Eyebrow, Icon, Input, Loading, Modal, PageHead, Panel, PanelHead, day, useApp } from "../ui.jsx";

export default function Members() {
  const { session, slug, notify, me } = useApp();
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  const [email, setEmail] = useState("");
  const [inviting, setInviting] = useState(false);
  const [removing, setRemoving] = useState(null);
  const [busy, setBusy] = useState(null);
  const reload = useCallback(
    () =>
      platform(session, `/companies/${slug}/members`).then(
        (d) => {
          setData(d);
          setError(null);
        },
        (e) => setError(e.message),
      ),
    [session, slug],
  );
  useEffect(() => {
    reload();
  }, [reload]);
  const owner = data?.your_role === "owner";
  const act = async (id, fn, msg) => {
    setBusy(id);
    try {
      await fn();
      notify(msg);
      reload();
    } catch (e) {
      notify(e.message, "error");
    }
    setBusy(null);
  };
  const invite = async (e) => {
    e.preventDefault();
    setInviting(true);
    try {
      const r = await platform(session, `/companies/${slug}/invites`, { method: "POST", body: { email } });
      setEmail("");
      notify(r.next);
      reload();
    } catch (err) {
      notify(err.message, "error");
    }
    setInviting(false);
  };
  const remove = async () => {
    const m = removing;
    await act(m.user_id, () => platform(session, `/companies/${slug}/members/${m.user_id}`, { method: "DELETE" }), `${m.email} removed.`);
    setRemoving(null);
  };

  return (
    <>
      <PageHead kicker="Two-factor on every account" title="Members" />
      <p className="reveal -mt-2 mb-6 max-w-2xl text-sm text-mute">People who can manage this company. Everyone uses two-factor authentication.</p>
      <ErrorNote error={error} />
      {!data && !error && <Loading rows={4} />}
      {data && (
        <div className="grid items-start gap-4 md:grid-cols-5">
          <Panel className="p-6 md:col-span-3">
            <PanelHead
              eyebrow="Can manage this company"
              title={`${data.members.length} ${data.members.length === 1 ? "member" : "members"}`}
              action={data.invites.length > 0 && <Badge tone="amber">{data.invites.length} invited</Badge>}
            />
            <ul className="divide-y divide-white/7">
              {data.members.map((m) => {
                const you = m.user_id === me?.user?.id;
                return (
                  <li key={m.user_id} className="flex items-center gap-3 py-3">
                    <Initial email={m.email} />
                    <div className="min-w-0 flex-1">
                      <p className="flex min-w-0 items-center gap-2 text-sm">
                        <span className="truncate" title={m.email}>{m.email}</span>
                        {you && <Badge>you</Badge>}
                      </p>
                      <p className="mt-0.5 text-xs text-mute">
                        <span className={m.role === "owner" ? "text-lime" : ""}>{m.role === "owner" ? "Owner" : "Admin"}</span> · since {day(m.created_at)}
                      </p>
                    </div>
                    {owner && m.role !== "owner" && (
                      <button
                        type="button"
                        onClick={() => setRemoving(m)}
                        disabled={busy === m.user_id}
                        title="Remove"
                        aria-label={`Remove ${m.email}`}
                        className="shrink-0 rounded-full p-2 text-mute transition hover:bg-ember/10 hover:text-ember disabled:opacity-40"
                      >
                        <RemoveIcon />
                      </button>
                    )}
                  </li>
                );
              })}
            </ul>
            {data.invites.length > 0 && (
              <>
                <Eyebrow className="mt-6 mb-1">Invitations</Eyebrow>
                <ul className="divide-y divide-white/7">
                  {data.invites.map((i) => (
                    <li key={i.id} className="flex items-center gap-3 py-3">
                      <Initial email={i.email} pending />
                      <div className="min-w-0 flex-1">
                        <p className="flex min-w-0 items-center gap-2 text-sm">
                          <span className="truncate" title={i.email}>{i.email}</span>
                          <Badge tone="amber" dot>invited</Badge>
                        </p>
                        <p className="mt-0.5 text-xs text-mute">Invited {day(i.created_at)} · waiting for them to sign up</p>
                      </div>
                      {owner && (
                        <button
                          type="button"
                          onClick={() => act(i.id, () => platform(session, `/companies/${slug}/invites/${i.id}`, { method: "DELETE" }), "Invitation cancelled.")}
                          disabled={busy === i.id}
                          title="Cancel invitation"
                          aria-label={`Cancel invitation for ${i.email}`}
                          className="shrink-0 rounded-full p-2 text-mute transition hover:bg-white/5 hover:text-fog disabled:opacity-40"
                        >
                          <Icon name="x" />
                        </button>
                      )}
                    </li>
                  ))}
                </ul>
              </>
            )}
          </Panel>

          <Panel i={1} className="p-6 md:col-span-2">
            <PanelHead eyebrow={owner ? "Owner only" : "Read only"} title="Invite someone" />
            <p className="mb-4 text-sm text-mute">
              {owner ? "They get admin access: approvals, the cap, hiring, and integrations." : "Only the owner can invite people."}
            </p>
            <form onSubmit={invite} className="space-y-4">
              <Input
                label="Email"
                type="email"
                value={email}
                onChange={setEmail}
                disabled={!owner}
                autoComplete="off"
                placeholder="name@company.com"
                className="[&_input:disabled]:cursor-not-allowed [&_input:disabled]:opacity-50"
              />
              <Button type="submit" disabled={!owner || inviting || !email.includes("@")}>
                <Icon name="plus" size={16} /> {inviting ? "Inviting…" : "Invite"}
              </Button>
            </form>
            <p className="mt-4 text-xs text-dim">Combox emails them how to join. They sign up with that address; the invitation has no link to forward.</p>
          </Panel>
        </div>
      )}
      <Modal open={!!removing} onClose={() => setRemoving(null)} kicker="Are you sure?" title={`Remove ${removing?.email ?? ""}?`}>
        <p className="text-sm text-mute">They lose admin access to this company right away.</p>
        <div className="mt-6 grid grid-cols-2 gap-3">
          <Button variant="ghost" onClick={() => setRemoving(null)}>Cancel</Button>
          <Button variant="danger" onClick={remove} disabled={busy === removing?.user_id}>
            {busy === removing?.user_id ? "Removing…" : "Remove"}
          </Button>
        </div>
      </Modal>
    </>
  );
}

// People are humans, so sky; an invitation is a dashed outline until they join.
function Initial({ email, pending = false }) {
  return (
    <span
      className={`flex shrink-0 items-center justify-center rounded-full font-display text-xs font-semibold size-9 ${
        pending ? "border border-dashed border-white/20 text-mute" : "bg-sky/15 text-sky ring-1 ring-inset ring-sky/30"
      }`}
      aria-hidden="true"
    >
      {email[0].toUpperCase()}
    </span>
  );
}

function RemoveIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM2 21v-1a6 6 0 0 1 12 0v1M16 11h6" />
    </svg>
  );
}
