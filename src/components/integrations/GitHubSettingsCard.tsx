import { useCallback,useEffect,useState,type FormEvent } from "react";
import { github,type GithubDeviceAuthorization } from "../../integrations/github";
import type { IntegrationStatus } from "../../integrations/types";
import { CredentialField,IntegrationCard } from "./IntegrationCard";
export function GitHubSettingsCard(){
 const [status,setStatus]=useState<IntegrationStatus|null>(null),[clientId,setClientId]=useState(""),[device,setDevice]=useState<GithubDeviceAuthorization|null>(null),[message,setMessage]=useState(""),[error,setError]=useState(""),[loading,setLoading]=useState(true);
 const refresh=useCallback(async()=>{setLoading(true);setError("");try{setStatus(await github.status())}catch(e){setError(String(e))}finally{setLoading(false)}},[]);useEffect(()=>{void refresh()},[refresh]);
 const save=async(e:FormEvent)=>{e.preventDefault();setError("");try{await github.saveClientId(clientId);setClientId("");setMessage("Client ID saved locally.");await refresh()}catch(r){setError(String(r))}};
 const connect=async()=>{setError("");try{const next=await github.beginDeviceFlow();setDevice(next);setMessage("Enter the code on GitHub, then wait here.");window.open(next.verificationUri,"_blank");const account=await github.pollDeviceFlow();setDevice(null);setMessage(`Connected as ${account}.`);await refresh()}catch(r){setError(String(r))}};
 const disconnect=async()=>{if(!window.confirm("Disconnect GitHub from Theta?"))return;try{await github.disconnect();setDevice(null);setMessage("GitHub disconnected.");await refresh()}catch(r){setError(String(r))}};
 return <IntegrationCard name="GitHub" category="DEVELOPMENT" description="Read repositories, notifications, issues, and pull requests; protected actions always request approval." configured={status?.configured} connected={status?.connected} loading={loading} error={error} message={message} accountLabel={status?.accountLabel} onSubmit={save}>
  <CredentialField label="GitHub OAuth Client ID" value={clientId} onChange={setClientId} placeholder={status?.configured?"Enter a new ID to replace it":"Paste Client ID"} required={!status?.configured}/>
  {device&&<p className="integration-message">Open {device.verificationUri} and enter code <b>{device.userCode}</b>.</p>}
  {status?.message&&<small>{status.message}</small>}<div className="button-row"><button disabled={loading||!clientId.trim()}>Save Client ID</button><button type="button" className="primary" disabled={loading||!status?.configured||Boolean(device)} onClick={()=>void connect()}>Connect</button>{status?.connected&&<button type="button" className="danger" onClick={()=>void disconnect()}>Disconnect</button>}</div>
 </IntegrationCard>;
}
