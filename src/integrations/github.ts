import { invoke } from "@tauri-apps/api/core";
import type { IntegrationStatus } from "./types";
export interface GithubDeviceAuthorization { userCode: string; verificationUri: string; expiresAt: number; intervalSeconds: number }
export const github = {
  status: () => invoke<IntegrationStatus>("github_status"), saveClientId: (clientId:string)=>invoke<void>("github_set_client_id",{clientId}),
  beginDeviceFlow:()=>invoke<GithubDeviceAuthorization>("github_begin_device_flow"), pollDeviceFlow:()=>invoke<string>("github_poll_device_flow"), disconnect:()=>invoke<void>("github_disconnect"),
  profile:()=>invoke<unknown>("github_get_profile"), repositories:(perPage=30)=>invoke<unknown>("github_list_repositories",{perPage}), notifications:(perPage=30)=>invoke<unknown>("github_list_notifications",{perPage}),
  searchIssues:(query:string,perPage=30)=>invoke<unknown>("github_search_issues",{query,perPage}), createIssue:(owner:string,repo:string,title:string,body?:string)=>invoke<unknown>("github_create_issue",{owner,repo,title,body}),
  commentIssue:(owner:string,repo:string,issueNumber:number,body:string)=>invoke<unknown>("github_comment_issue",{owner,repo,issueNumber,body}),
};
