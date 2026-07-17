import { CheerioCrawler } from 'crawlee';
import TurndownService from 'turndown'
const turndownService = new TurndownService({
    headingStyle: 'atx',
    codeBlockStyle: 'fenced'
});

async function makeRequest(query:string, ask:(param:string)=>Promise<string>) {
  const data = JSON.stringify({
    "q": "apple inc"
  });

  const config = {
    method: 'POST',
    headers: { 
      'X-API-KEY': '', 
      'Content-Type': 'application/json'
    },
    body: data
  };
  
    const response = await fetch('https://google.serper.dev/search', config);
    
    if (!response.ok) {
      return "";
    }
    let urls:string[] = [];
    const result = await response.json();
    result.organic.forEach((value:any)=>{urls.push(value.link)});
    const markdownPages: string[] = [];
    const crawler = new CheerioCrawler({
        async requestHandler({ request, $, log }) {
            log.info(`Converting to Markdown: ${request.url}`);
            $('script, style, noscript, iframe, nav, footer, header').remove();

            const htmlContent = $('body').html() || '';
            const markdown = turndownService.turndown(htmlContent);

            const pageMarkdown = `\n\n# Source: ${request.url}\n\n${markdown}`;
            markdownPages.push(pageMarkdown);
        },
        maxRequestsPerCrawl: 10, 
    });
    await crawler.run(urls);

    const combinedMarkdown = markdownPages.join('\n\n---');
    const ret = await ask("Summarize these web results: " + combinedMarkdown);
    return ret;
}