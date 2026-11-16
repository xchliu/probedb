# 海外公有云厂商：IDC / 数据中心相关成本占比基准（US 视角）

> 研究问题：公有云服务中，IDC 运营费用一般占多少？
> 本文口径：海外公有云厂商（AWS / Microsoft Azure / Google Cloud / Oracle OCI）的财报披露 + 机构估算。
> **重要前提：四家厂商均不单独披露"电力/租金/折旧"等 IDC 明细科目占比。** 财报只到 segment 收入/营业利润、以及 cost of revenue 总额。因此本文分两层：
> - **硬数据（公司披露）**：收入、营业利润/毛利率、成本总额、capex、管理层定性表述；
> - **估算（标明推导链条）**：IDC 相关成本在 COGS / 收入中的拆解。
> 所有"估算"标记项均为本文基于公开硬数据的推算或引用第三方估算，**不是公司披露值**。

---

## 1. 汇总表

| 对象 | 年份 | 指标 | 数值 / 区间 | 口径 | 来源 |
|---|---|---|---|---|---|
| **Amazon AWS** | FY2025 | 收入 | **$128.7B**（+20% YoY） | 公司披露，segment 净销售额 | [Amazon Q4/FY2025 results](https://www.jamessharp.co.uk/market-news/amazon-com-announces-q4-2025-results/) |
| **Amazon AWS** | FY2025 | 营业利润 / 营业利润率 | **$45.6B / 35.4%**（FY2024：$39.8B / 37.0%） | 公司披露，segment operating income（未分配部分公司费用） | 同上 |
| **Amazon AWS** | FY2025 | 由此倒推的 AWS 总成本（含 COGS+可归属费用） | **≈$83.1B，占 AWS 收入 64.6%** | 推算：收入−营业利润 | 本文推算（同上数据） |
| **Amazon 集团** | FY2025 | CapEx 指引 | **2026 年约 $200B**（集团口径，主要投 AI） | 公司披露（Jassy 原话） | 同上 |
| **Amazon 集团** | FY2024 | 折旧摊销（D&A） | **$52.5B**（全集团，未拆 AWS） | 公司披露，现金流量表 | [Amazon 10-K FY2024](https://www.sec.gov/Archives/edgar/data/1018724/000101872425000004/0001018724-25-000004.txt) ※本次抓取被 SEC 反爬拦截，数值为财报公开值，明细未逐项核 |
| **Amazon AWS** | FY2024–25 | IDC 相关成本在 AWS COGS 中的结构 | **估算**：折旧约 **45–60%**；电力+房租/设施运维约 **10–20%**；带宽/网络 transit 约 **5–10%**；运维人员约 **10–15%**；其他（软件/支持）约 **10–20%** | 估算（依据 10-K 对 AWS cost of sales 的文字描述：含服务器/网络设备折旧、电力、租金、带宽、人员；比例为本报告推算） | 本文推算；描述依据 [Amazon 10-K](https://www.sec.gov/Archives/edgar/data/1018724/000101872425000004/0001018724-25-000004.txt) |
| **Microsoft** | FY2025 | 收入 / 营业利润 | **$281.7B / $128.5B**（营业利润率 45.6%） | 公司披露 | [Microsoft Annual Report 2025](https://www.microsoft.com/investor/reports/ar25/index.html) |
| **Microsoft** | FY2025 | Cost of revenue | **≈$87.8B，占收入 31.2%**（公司口径：同比增长 $13.7B / +19%，由 Microsoft Cloud 增长驱动）；对应毛利率约 68.8% | 公司披露 + 简单推算 | 同上 |
| **Microsoft** | FY2025 | Microsoft Cloud gross margin | **约 68%**（FY2024 为 69%，云毛利率因 AI 基础设施投入略降） | 公司口径（财报电话会/年报） | 同上；[MarketWatch 报道 Azure 毛利率略降](https://www.marketwatch.com/livecoverage/microsoft-earnings-stock-results-azure-guidance-ai/card/microsoft-s-cloud-business-gets-a-bit-less-profitable-mRN7MCWeBSCT3fsRnQVD) |
| **Microsoft** | FY2025 | 数据中心规模 | **>400 个数据中心 / 70 个区域**；单年新增 **>2GW** 容量；2024 年可再生能源签约 34GW | 公司披露（规模指标，用于校准每 GW 收入） | [Microsoft AR25](https://www.microsoft.com/investor/reports/ar25/index.html) |
| **Microsoft** | FY2025 | Azure 收入 | **>$75B，+34%** | 公司披露 | 同上 |
| **Google Cloud** | FY2024 | 收入 / 营业利润 / 营业利润率 | **$43.2B / $6.1B / 14.1%** | 公司披露，Alphabet segment | Alphabet 10-K FY2024（Google Cloud segment）；2025 全年数字未在本文逐项核实 |
| **Google Cloud** | FY2025 | 收入趋势 | 增速回升至约 **+30%** 量级（AI 需求驱动）；未披露电力/折旧占比 | 公司口径，定性 | [Alphabet Q4 2025 电话会](https://www.fool.com/earnings/call-transcripts/2026/02/04/alphabet-googl-q4-2025-earnings-call-transcript/) |
| **Oracle OCI** | FY2025–26 | 云基础设施毛利率 | **未单独披露**。管理层口径：OCI 毛利率处于 **30%+ 区间**，长期目标 50%+（**媒体转述/定性，低置信度**） | 估算/定性 | [Oracle Investor Day 评论](https://finimize.com/content/oracles-investor-day-puts-its-ai-cloud-margins-to-the-test) |
| **Oracle** | FY2026 | CapEx 指引 | 约 **$35B** 量级（AI 数据中心军备竞赛） | 公司指引（媒体转述） | 同上 |
| **数据中心（自建/自运营设施）** | 2026 | 年度 OPEX 结构（**不含服务器折旧**） | 电力 **45–60%**；冷却/机械维护 **15–20%**；人员 **15–25%**；硬件维护与备件 **10–15%**；网络与软件工具 **5–10%** | 第三方行业指南（汇总 Uptime Institute / CBRE / IEA / Gartner） | [AptlyTech《Data Center Operating Costs 2026》](https://www.aptlytech.com/data-center-operating-costs-2026/) |
| **数据中心（自建/自运营设施）** | 2026 | 单位 OPEX | 标准企业设施 **$1.0–1.8M/MW/年**；高密度 AI 设施 **$2.2M+/MW/年**；月度 $120–250/kW（受限市场 $280–420/kW） | 同上 | 同上 |
| **数据中心（电费占总成本）** | — | 电费占比 | 电费约占数据中心**总成本基数（含资本）的 20%** | 机构估算（麦肯锡） | [McKinsey on Investing No.10](https://www.mckinsey.com/~/media/mckinsey/industries/private%20equity%20and%20principal%20investors/our%20insights/mckinsey%20on%20investing%20issue%2010/mckinsey-on-investing-number-10-full-issue.pdf) |
| **批发型 IDC（租户视角）** | 2025 | 机柜租金基准 | 批发型（4+MW）**约 $100–200/kW/月，且不含电费**（电力由租户按实付/转嫁另计） | 行业招股书披露（NTT DC REIT） | [NTT DC REIT 招股书 2025](https://links.sgx.com/FileOpen/NTT%20DC%20REIT%20-%20Registration%20Prospectus_7%20July%202025_final.ashx?App=Prospectus&FileID=66695) |

---

## 2. 结论区间

### 2.1 口径 A：**仅 IDC 运营费用（电力 + 房租/设施 + 设施运维 + 带宽），不含任何设备折旧**

| 分母 | 典型区间 | 依据 |
|---|---|---|
| 占云收入 | **约 10%–20%** | 自建型厂商电力约占收入 2%–5%、租金/设施 3%–6%、设施与运维人员 2%–4%、带宽 3%–5%（本文估算，基于 AWS/Azure 成本结构与单位电耗倒推） |
| 占云 COGS | **约 15%–30%** | 同上；AWS COGS ≈ 收入的 65%，Microsoft COGS ≈ 收入的 31% |

> 这是最接近用户问题"IDC 运营费用占比"的口径。**给业务方的简化答案：海外头部公有云，纯 IDC 运营费用（电、租、设施运维、带宽）大致占云收入的 10%–20%、占云成本的 15%–30%。**

### 2.2 口径 B：**IDC 全生命周期成本 = 运营费用 + 服务器/网络/机电设备折旧**

| 分母 | 典型区间 | 依据 |
|---|---|---|
| 占云收入 | **约 40%–55%** | AWS：35% 营业利润率意味着 65% 成本率；其中折旧（估算占 COGS 45%–60%）≈ 收入的 30%–40%，叠加运营 10%–20% |
| 占云 COGS | **约 60%–80%** | 同上；Microsoft 因软件占比高，该比例显著更低（**约 45%–60%**） |

### 2.3 口径 C：**纯设施侧（电 + 冷却 + 机房租金/机电折旧），不含 IT 设备**

- 占云收入：**约 8%–15%**（自建型）；
- 占云 COGS：**约 12%–23%**；
- 设施侧内部结构（AptlyTech/Uptime 口径）：电力 45%–60%，冷却 15%–20%，人员 15%–25%，硬件维护 10%–15%，网络 5%–10%。

### 2.4 关键提醒：三个口径差异巨大（1.5–2.5 倍）

对外沟通时必须先定义口径：
- "IDC 运营费用"= **只电费租金运维** → 收入 10%–20%；
- "IDC 相关成本"= **含设备折旧** → 收入 40%–55%；
- 若把"整个云 COGS"当 IDC 成本，会得到 65%–70%（AWS）/ 31%（Microsoft），**这是错误映射**，因为 COGS 里还含带宽采购、第三方软件、客服/运维人力、支付渠道等。

---

## 3. 自建 vs 租用批发型 IDC 的差异

### 3.1 自建（AWS / Azure / Google / Meta 主流模式）

- 成本主导项：**设备折旧**（服务器通常 5–6 年、GPU 6 年、网络设备 5 年），其次电力与设施；
- 资本前置：capex → 折旧 → 摊入 COGS，**利润率在建设期被压低、在折旧末期改善**；
- 电费占比随 PUE 与电价波动：PUE 1.1 与 1.5 的设施，电费差 30%–40%；
- 自有资产使"租金"项趋近于 0，被"机电折旧 + 土地/建筑折旧"替代。

### 3.2 租用批发型 IDC（中小云 / 托管云 / AI 新云常见）

- 成本主导项：**机柜租金 + 电费（转嫁）**，二者合计通常占 IDC 相关成本的 **60%–80%**；
- 租金基准：批发型约 **$100–200/kW/月（不含电）**，电力单独按 kWh 实付或转嫁；
- 折旧只剩 IT 设备（服务器/GPU/网络），不含机电；
- 对应占收入比例更高（因为向第三方支付了 landlord 的资本回报 + 毛利）：**IDC 租金+电费常占云收入的 25%–40%**（估算，中小云/托管场景）；
- 合同风险：长约 5–15 年 take-or-pay，形成准固定成本。

### 3.3 混合模式

大厂也在租（如 Microsoft/AWS 在部分区域租用 colo/批发 IDC 以加快交付），财报中体现为租金费用（operating lease），不体现为折旧。这也是财报无法直接给出"IDC 成本占比"的原因之一。

---

## 4. AI 数据中心对占比的影响（2023 → 2026）

| 趋势 | 方向 | 证据 |
|---|---|---|
| 单 MW 造价与 OPEX 上升 | AI 高密度机柜（40–132kW）**$2.2–2.85M/MW/年**，比传统企业设施（$1.1–1.4M/MW/年）高 **60%–100%** | AptlyTech 2026 |
| 折旧占 COGS 上升 | capex 暴增（Amazon 2026 指引 $200B，Oracle FY2026 ~$35B），**折旧前置**，云毛利率承压 | Amazon / Oracle 指引 |
| 云毛利率下行 | Microsoft Cloud gross margin 由 69%（FY2024）→ 约 68%（FY2025），公司归因于 AI 基础设施扩张 | Microsoft AR25 |
| 电力从"小项"变"战略项" | 电力受限市场（北弗吉尼亚、法兰克福、硅谷）月度 OPEX 达 $280–420/kW，是普通市场 1.7–2.3 倍 | AptlyTech 2026 |
| 折旧年限与残值争议 | GPU 6 年、服务器 5–6 年的假设直接影响折旧率；缩短年限会**一次性抬高 IDC 成本占比** | 行业普遍假设 |
| 收入端 | AI 推理收入增长快于成本摊薄 → 短期占比上行、中期若利用率提升则回落（不确定） | 估算 |

**结论：2023–2026 年，IDC 相关成本占云收入的比例整体呈上升趋势（估算 +5–10pp），驱动因素排序：① 折旧前置（capex 增速 > 收入增速）；② AI 机柜的电力密度与冷却成本；③ 电力价格与并网成本上升。**

---

## 5. 不确定性与局限

1. **四家厂商均未披露**电力、租金、折旧分项与云业务 COGS 明细；本文 AWS 结构拆分为推算，误差可能达 ±10pp。
2. Microsoft 只披露"Microsoft Cloud gross margin"，Azure 毛利率与折旧未单独披露；Google 只披露 Google Cloud segment operating income。
3. Oracle OCI 毛利率无独立科目，本文仅引用媒体转述的管理层定性，**置信度低**。
4. Google Cloud FY2025 全年硬数据未在本文逐项核实（写"约+30%"为公司口径定性）。
5. Amazon 10-K 原文本次抓取被 SEC 反爬拦截（HTTP 403），FY2024 D&A $52.5B 为财报公开值但未逐项核对页面；建议以 10-K 原文复核。
6. 行业 OPEX 结构（电力 45–60% 等）来自第三方咨询指南，汇总自 Uptime Institute/CBRE/IEA，**非厂商财报**，代表"数据中心运营商"视角而非"公有云厂商"视角。

---

## 6. 参考来源

**公司披露**
1. [Amazon.com Announces Q4 2025 Results（FY2025：AWS $128.7B / 营业利润 $45.6B / 2026 capex ~$200B）](https://www.jamessharp.co.uk/market-news/amazon-com-announces-q4-2025-results/)
2. [Amazon Form 10-K FY2024（AWS cost of sales 构成描述、D&A）](https://www.sec.gov/Archives/edgar/data/1018724/000101872425000004/0001018724-25-000004.txt)
3. [Microsoft Annual Report 2025（收入 $281.7B、营业利润 $128.5B、cost of revenue +19%、Azure >$75B、>400 DC）](https://www.microsoft.com/investor/reports/ar25/index.html)
4. [Alphabet Q4 2025 Earnings Call Transcript](https://www.fool.com/earnings/call-transcripts/2026/02/04/alphabet-googl-q4-2025-earnings-call-transcript/)

**行业 / 机构估算**
5. [AptlyTech, *Data Center Operating Costs: 2026 Cost & TCO Guide*（OPEX 结构、$/MW/年、$/kW/月）](https://www.aptlytech.com/data-center-operating-costs-2026/)
6. [McKinsey on Investing No.10（电费约占数据中心总成本 20%）](https://www.mckinsey.com/~/media/mckinsey/industries/private%20equity%20and%20principal%20investors/our%20insights/mckinsey%20on%20investing%20issue%2010/mckinsey-on-investing-number-10-full-issue.pdf)
7. [NTT DC REIT Registration Prospectus 2025（批发型 IDC 租金 $/kW/月，不含电费）](https://links.sgx.com/FileOpen/NTT%20DC%20REIT%20-%20Registration%20Prospectus_7%20July%202025_final.ashx?App=Prospectus&FileID=66695)
8. [Uptime Institute Research（数据中心可用性与成本基准）](https://uptimeinstitute.com/resources/research-and-reports)
9. [IEA, Data centres and data transmission networks](https://www.iea.org/energy-system/buildings/data-centres-and-data-transmission-networks)
10. [MarketWatch：Microsoft 云业务毛利率略降](https://www.marketwatch.com/livecoverage/microsoft-earnings-stock-results-azure-guidance-ai/card/microsoft-s-cloud-business-gets-a-bit-less-profitable-mRN7MCWeBSCT3fsRnQVD)
11. [Finimize：Oracle Investor Day 与 AI 云毛利率](https://finimize.com/content/oracles-investor-day-puts-its-ai-cloud-margins-to-the-test)

---

*撰写：global-cloud-researcher ｜ 数据截止：2026-11-16 ｜ 硬数据与估算已逐项标注口径*
